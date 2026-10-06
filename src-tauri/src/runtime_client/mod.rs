//! The IDE's connection to the PLC runtime. `RuntimeProcess` owns the `myplc-runtime
//! --serve` child process (start, readiness check, clean shutdown); `RuntimeClient` is the
//! client: it sends protocol requests over the child's stdin, one JSON line each, and
//! reads responses (matched to their request by id) and `STATE_UPDATE` events from its
//! stdout. Every state it learns, from either, goes to the event listener (the IDE pushes
//! it to the UI) and answers GET_STATE without a round trip.
//!
//! The runtime is started when first needed. If it dies, the listener hears `Lost` (and the
//! next state request reports UNAVAILABLE); the next command starts a new one. When the IDE goes away the child's stdin closes, so it stops the PLC and exits
//! even if the IDE couldn't shut it down: no orphan processes. There is no other PLC: if
//! the runtime can't be started or reached, requests fail with UNAVAILABLE.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use myplc_core::protocol::{Command, ErrorCode, ProtocolError, Request, RequestId, Response, ResponseData, RuntimeMessage, RuntimeState};

/// The longest a request may take (LOAD_PROJECT compiles the whole project).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest a starting runtime may take to answer its first request.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a closing runtime gets to stop the PLC and exit before it is killed.
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);

/// The `myplc-runtime` executable the IDE starts. It always sits next to the IDE's own
/// executable: in development `npm run dev` builds it into the same `target/` folder, and
/// an installed MyPLC has it bundled beside `my-plc-ide.exe` (Tauri `externalBin`, see
/// `tauri.bundle.conf.json`). Tests run one folder deeper, in `target/*/deps`.
/// `MYPLC_RUNTIME` overrides it (for development). Never a PATH lookup: an installed IDE
/// must start its own runtime, not whichever one a search finds.
pub fn resolve_runtime_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("MYPLC_RUNTIME") {
        return path.into();
    }
    let name = format!("myplc-runtime{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().unwrap_or_default();
    let candidates: Vec<PathBuf> = exe.ancestors().skip(1).take(2).map(|dir| dir.join(&name)).collect();
    // Not found: where it should be, so starting it fails with a clear error.
    candidates.iter().find(|path| path.is_file()).or(candidates.first()).cloned().unwrap_or_else(|| name.into())
}

fn unavailable(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::Unavailable, message)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the runtime tells the IDE without being asked.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeEvent {
    /// The PLC state changed: a `STATE_UPDATE`, or the state a command left it in. In the
    /// order the runtime sent them, so the last one is the current state.
    State(Box<RuntimeState>),
    /// The runtime stopped unexpectedly (`UNAVAILABLE`).
    Lost(ProtocolError),
}

/// Receives `RuntimeEvent`s, on the thread that reads the runtime: it must not block.
pub type EventListener = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;

/// What the reader thread shares with the client.
struct Shared {
    /// Requests waiting for their response, by id.
    pending: Mutex<HashMap<RequestId, Sender<Response>>>,
    /// The state from the latest response or event.
    latest: Mutex<Option<RuntimeState>>,
    connected: AtomicBool,
    /// Set when the client closes the connection itself: then the end isn't a loss.
    closing: AtomicBool,
    events: Option<EventListener>,
}

impl Shared {
    /// Every message from the runtime, until its stdout closes.
    fn read(&self, stdout: ChildStdout) {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            match serde_json::from_str::<RuntimeMessage>(&line) {
                Ok(RuntimeMessage::StateUpdate { state }) => self.learned(state),
                Ok(RuntimeMessage::Response(response)) => {
                    if let Ok(data) = &response.result {
                        self.learned(data.state.clone());
                    }
                    let waiting = response.id.as_ref().and_then(|id| lock(&self.pending).remove(id));
                    if let Some(waiting) = waiting {
                        let _ = waiting.send(response);
                    }
                }
                Err(e) => eprintln!("[runtime client] unreadable message from the runtime: {e}"),
            }
        }
        // Disconnected: whoever still waits hears it now (their senders drop).
        let mut pending = lock(&self.pending);
        self.connected.store(false, Ordering::SeqCst);
        pending.clear();
        drop(pending);
        if !self.closing.load(Ordering::SeqCst) {
            self.notify(RuntimeEvent::Lost(unavailable("The PLC runtime stopped unexpectedly.")));
        }
    }

    /// The runtime reported its state: keep it for GET_STATE, pass it on.
    fn learned(&self, state: RuntimeState) {
        *lock(&self.latest) = Some(state.clone());
        self.notify(RuntimeEvent::State(Box::new(state)));
    }

    fn notify(&self, event: RuntimeEvent) {
        if let Some(events) = &self.events {
            events(event);
        }
    }
}

/// A running `myplc-runtime --serve` and the connection to it.
struct RuntimeProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    shared: Arc<Shared>,
}

/// Why a request got no answer from the runtime.
struct Lost(&'static str);

impl RuntimeProcess {
    /// START → CONNECT → READY: spawn the runtime and wait for its first answer.
    fn start(program: &PathBuf, ids: &AtomicU64, events: Option<EventListener>) -> Result<Self, ProtocolError> {
        let mut command = std::process::Command::new(program);
        command.arg("--serve").stdin(Stdio::piped()).stdout(Stdio::piped());
        // Its log (stderr) shows in the IDE's console; tests keep theirs quiet.
        command.stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000; // no console window from the GUI app
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| {
            // The details are for development (stderr); the user gets a plain sentence.
            eprintln!("[runtime client] can't start {}: {e}", program.display());
            unavailable(if e.kind() == std::io::ErrorKind::NotFound {
                "The PLC runtime (myplc-runtime) was not found. Reinstall MyPLC to restore it."
            } else {
                "The PLC runtime could not be started."
            })
        })?;
        let stdout = child.stdout.take().expect("piped");
        let shared = Arc::new(Shared {
            pending: Mutex::default(),
            latest: Mutex::default(),
            connected: AtomicBool::new(true),
            closing: AtomicBool::new(false),
            events,
        });
        let reader = Arc::clone(&shared);
        thread::Builder::new().name("runtime-reader".into()).spawn(move || reader.read(stdout)).map_err(|_| {
            let _ = child.kill();
            unavailable("The PLC runtime could not be started.")
        })?;
        let mut process = RuntimeProcess { stdin: child.stdin.take(), child, shared };
        match process.request(ids, Command::GetState, READY_TIMEOUT) {
            Ok(_) => Ok(process),
            Err(_) => {
                process.close();
                Err(unavailable("The PLC runtime did not start correctly."))
            }
        }
    }

    fn connected(&self) -> bool {
        self.shared.connected.load(Ordering::SeqCst)
    }

    /// Send one request and wait for its response.
    fn request(&mut self, ids: &AtomicU64, command: Command, timeout: Duration) -> Result<Response, Lost> {
        let id = ids.fetch_add(1, Ordering::Relaxed).to_string();
        let (send, answer) = mpsc::channel();
        {
            let mut pending = lock(&self.shared.pending);
            if !self.connected() {
                return Err(Lost("The PLC runtime stopped unexpectedly."));
            }
            pending.insert(id.clone(), send);
        }
        let line = serde_json::to_string(&Request { id: id.clone(), command }).expect("requests serialize");
        let written = self.stdin.as_mut().map(|stdin| writeln!(stdin, "{line}").and_then(|()| stdin.flush()));
        if !matches!(written, Some(Ok(()))) {
            lock(&self.shared.pending).remove(&id);
            return Err(Lost("The PLC runtime stopped unexpectedly."));
        }
        match answer.recv_timeout(timeout) {
            Ok(response) => Ok(response),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.shared.pending).remove(&id);
                Err(Lost("The PLC runtime is not responding."))
            }
            Err(RecvTimeoutError::Disconnected) => Err(Lost("The PLC runtime stopped unexpectedly.")),
        }
    }

    /// STOP → DISCONNECT: closing its input makes the runtime stop the PLC and exit; one
    /// that doesn't in time is killed. Either way the process is reaped.
    fn close(mut self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        drop(self.stdin.take());
        let deadline = Instant::now() + EXIT_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct RuntimeClient {
    program: PathBuf,
    process: Mutex<Option<RuntimeProcess>>,
    ids: AtomicU64,
    events: Option<EventListener>,
}

impl RuntimeClient {
    pub fn new(program: PathBuf) -> Self {
        Self { program, process: Mutex::new(None), ids: AtomicU64::new(1), events: None }
    }

    /// Tell `listener` about every state change and an unexpected end of the runtime, of
    /// this runtime and of any it is restarted as.
    pub fn with_events(mut self, listener: impl Fn(RuntimeEvent) + Send + Sync + 'static) -> Self {
        self.events = Some(Arc::new(listener));
        self
    }

}

impl RuntimeClient {
    /// One protocol request from the UI, answered with its response (same id). The client
    /// numbers its own requests to the runtime; the caller's id comes back unchanged.
    pub fn request(&self, request: Request) -> Response {
        Response { id: Some(request.id), result: self.execute(request.command).map(|state| ResponseData { state }) }
    }

    /// Carry out one command in the runtime: the state afterwards, or why it was refused
    /// (`UNAVAILABLE` when the runtime can't be started or reached).
    pub fn execute(&self, command: Command) -> Result<RuntimeState, ProtocolError> {
        let mut slot = lock(&self.process);
        let get_state = matches!(command, Command::GetState);
        if slot.as_ref().is_some_and(|p| !p.connected()) {
            // It died since the last request: a state request reports that once (the UI
            // has already heard it as an event); any other command starts a new runtime.
            slot.take().expect("checked").close();
            if get_state {
                return Err(unavailable("The PLC runtime stopped unexpectedly."));
            }
        }
        if get_state {
            if let Some(state) = slot.as_ref().and_then(|p| lock(&p.shared.latest).clone()) {
                return Ok(state);
            }
        }
        if slot.is_none() {
            *slot = Some(RuntimeProcess::start(&self.program, &self.ids, self.events.clone())?);
        }
        let process = slot.as_mut().expect("started");
        match process.request(&self.ids, command, REQUEST_TIMEOUT) {
            Ok(response) => response.result.map(|data| data.state),
            Err(Lost(message)) => {
                slot.take().expect("running").close();
                Err(unavailable(message))
            }
        }
    }

    /// The IDE is closing: stop the PLC and end the runtime process.
    pub fn shutdown(&self) {
        if let Some(process) = lock(&self.process).take() {
            process.close();
        }
    }
}

impl Drop for RuntimeClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}
