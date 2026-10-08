//! `myplc-runtime --listen <addr>`: the PLC runtime as a network server, meant to run 24×7
//! (e.g. as a service on an on-prem machine). IDEs connect over a WebSocket at
//! `ws://<addr>` and speak the MyPLC protocol (`myplc_core::protocol`), one JSON message
//! per text frame: requests in; responses (same id) and `STATE_UPDATE` events out.
//!
//! - Any number of clients; they all control and watch the same PLC.
//! - A client leaving does not stop the PLC.
//! - The loaded project and the RUN/STOP mode are saved in the data folder, so after a
//!   restart (crash, reboot) the same project is loaded and, if it was running, runs again.
//! - With a token (`--token` or `MYPLC_TOKEN`), clients must connect to `ws://<addr>/?token=…`.

use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request as HttpRequest, Response as HttpResponse};
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::host::Host;
use myplc_core::protocol::{Command, ProtocolError, Request, Response, RuntimeMessage, RuntimeState};
use myplc_core::ENGINE_STACK_BYTES;

/// How long a connection waits for a request before checking the state for an update.
const EVENT_INTERVAL: Duration = Duration::from_millis(50);
/// The file in the data folder that remembers the project and the RUN/STOP mode.
const SAVED_FILE: &str = "runtime-state.json";

pub struct ServerConfig {
    pub listen: String,
    pub token: Option<String>,
    pub data_dir: PathBuf,
}

/// The project and mode to restore after a restart.
struct Saved {
    dir: PathBuf,
}

impl Saved {
    fn path(&self) -> PathBuf {
        self.dir.join(SAVED_FILE)
    }

    fn read(&self) -> Option<Value> {
        serde_json::from_str(&fs::read_to_string(self.path()).ok()?).ok()
    }

    /// Remember the last project loaded and whether the PLC is meant to run.
    fn write(&self, project: Option<&Command>, running: Option<bool>) {
        let mut saved = self.read().unwrap_or_else(|| json!({}));
        if let Some(project) = project {
            saved["project"] = serde_json::to_value(project).expect("commands serialize");
        }
        if let Some(running) = running {
            saved["running"] = json!(running);
        }
        let written = fs::create_dir_all(&self.dir).and_then(|()| fs::write(self.path(), saved.to_string()));
        if let Err(e) = written {
            eprintln!("[runtime] can't save {}: {e}", self.path().display());
        }
    }

    /// Load the saved project again, and run it if it was running.
    fn restore(&self, host: &Host) {
        let Some(saved) = self.read() else { return };
        let Some(project) = saved.get("project").and_then(|p| serde_json::from_value::<Command>(p.clone()).ok()) else { return };
        match host.execute(project) {
            Ok(state) => eprintln!("[runtime] restored project: {}", state.programs.join(", ")),
            Err(e) => return eprintln!("[runtime] could not restore the saved project: {}", e.message),
        }
        if saved["running"] == json!(true) {
            match host.execute(Command::Run(None)) {
                Ok(_) => eprintln!("[runtime] RUNNING again after restart"),
                Err(e) => eprintln!("[runtime] could not run the restored project: {}", e.message),
            }
        }
    }
}

/// Decode one request; a message that isn't one gets a `REJECTED` answer (with its id,
/// when one can be read).
fn decode(text: &str) -> Result<Request, Box<Response>> {
    serde_json::from_str(text).map_err(|e| {
        let id = serde_json::from_str::<Value>(text).ok().and_then(|v| v.get("id")?.as_str().map(String::from));
        Box::new(Response::error(id, ProtocolError::invalid_request(e)))
    })
}

/// Carry out a request; persist what must survive a restart.
fn answer(host: &Host, saved: &Saved, request: Request) -> (Response, Option<RuntimeState>) {
    let Request { id, command } = request;
    // What to remember if it succeeds: a new project (loaded STOPPED), or the RUN/STOP mode.
    let remember = match &command {
        Command::LoadProject { .. } => Some((Some(command.clone()), false)),
        Command::Run(_) => Some((None, true)),
        Command::Stop => Some((None, false)),
        _ => None,
    };
    match host.execute(command) {
        Ok(state) => {
            if let Some((project, running)) = remember {
                saved.write(project.as_ref(), Some(running));
            }
            (Response::ok(id, state.clone()), Some(state))
        }
        Err(error) => (Response::error(Some(id), error), None),
    }
}

/// The `token` query parameter of a connection request, if any.
fn token_of(request: &HttpRequest) -> Option<&str> {
    request.uri().query()?.split('&').find_map(|pair| pair.strip_prefix("token="))
}

fn send(ws: &mut WebSocket<TcpStream>, message: &RuntimeMessage) -> Result<(), WsError> {
    ws.send(Message::text(serde_json::to_string(message).expect("protocol messages serialize")))
}

/// One client: answer its requests and push it every state change, until it leaves.
#[allow(clippy::result_large_err)] // the handshake callback's error type is tungstenite's
fn serve_client(stream: TcpStream, host: &Host, saved: &Saved, token: Option<&str>) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    let check = |request: &HttpRequest, response: HttpResponse| -> Result<HttpResponse, ErrorResponse> {
        match token {
            Some(expected) if token_of(request) != Some(expected) => {
                use tungstenite::http::{header, HeaderValue, StatusCode};
                let body = "A valid token is required.";
                let mut refused = ErrorResponse::new(Some(body.into()));
                *refused.status_mut() = StatusCode::UNAUTHORIZED;
                // A complete response, so every client sees the refusal at once.
                refused.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
                refused.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("close"));
                Err(refused)
            }
            _ => Ok(response),
        }
    };
    let mut ws = match tungstenite::accept_hdr(stream, check) {
        Ok(ws) => ws,
        Err(e) => return eprintln!("[runtime] refused {peer}: {e}"),
    };
    if ws.get_ref().set_read_timeout(Some(EVENT_INTERVAL)).is_err() {
        return;
    }
    eprintln!("[runtime] client connected: {peer}");
    let mut last_sent: Option<RuntimeState> = None;
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let (response, state) = match decode(&text) {
                    Ok(request) => answer(host, saved, request),
                    Err(rejected) => (*rejected, None),
                };
                if state.is_some() {
                    last_sent = state;
                }
                if send(&mut ws, &RuntimeMessage::Response(response)).is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {} // pings are answered by the library
            Err(WsError::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        // Push the state when it changed (scans ran, the debugger stopped, a scan failed).
        let state = host.state();
        if last_sent.as_ref() != Some(&state) {
            if send(&mut ws, &RuntimeMessage::StateUpdate { state: state.clone() }).is_err() {
                break;
            }
            last_sent = Some(state);
        }
    }
    eprintln!("[runtime] client left: {peer} (the PLC keeps its state and mode)");
}

pub fn serve(config: ServerConfig) -> ExitCode {
    let listener = match TcpListener::bind(&config.listen) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("[runtime] can't listen on {}: {e}", config.listen);
            return ExitCode::FAILURE;
        }
    };
    let host = Arc::new(Host::new());
    let saved = Arc::new(Saved { dir: config.data_dir.clone() });
    saved.restore(&host);
    let token: Option<Arc<str>> = config.token.map(Into::into);
    eprintln!(
        "[runtime] listening on ws://{} (data: {}{})",
        config.listen,
        config.data_dir.display(),
        if token.is_some() { ", token required" } else { ", no token: anyone who can reach this port controls the PLC" }
    );
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (host, saved, token) = (Arc::clone(&host), Arc::clone(&saved), token.clone());
        // Commands like STEP run scans on the client's thread: give it the engine's stack.
        let spawned = thread::Builder::new()
            .name("plc-client".into())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn(move || serve_client(stream, &host, &saved, token.as_deref()));
        if let Err(e) = spawned {
            eprintln!("[runtime] can't serve a client: {e}");
        }
    }
    ExitCode::SUCCESS
}

/// The default data folder: `myplc-data` next to the executable (a service's working
/// folder is often a system folder).
pub fn default_data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("myplc-data")
}
