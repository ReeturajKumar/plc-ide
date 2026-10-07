//! `myplc-runtime --serve`: the runtime as a service for one client over stdin/stdout,
//! one JSON protocol message per line (`myplc_core::protocol`). Requests come in on stdin;
//! responses and `STATE_UPDATE` events go out on stdout; logs go to stderr. End of input
//! means the client is gone: the PLC stops and the process exits.

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::host::Host;
use myplc_core::protocol::{Command, ProtocolError, Request, Response, RuntimeMessage, RuntimeState};

/// How often the state is checked for changes worth a `STATE_UPDATE`.
const EVENT_INTERVAL: Duration = Duration::from_millis(50);

/// stdout and the state the client last received. Locked for "read the state, write the
/// message" as one step, so the stream is in state order: a message never carries an
/// older state than the one before it.
struct Out {
    stdout: std::io::Stdout,
    last_sent: Option<RuntimeState>,
}

impl Out {
    fn send(&mut self, message: &RuntimeMessage) -> std::io::Result<()> {
        let line = serde_json::to_string(message).expect("protocol messages serialize");
        writeln!(self.stdout, "{line}")?;
        self.stdout.flush()
    }
}

fn lock(out: &Mutex<Out>) -> std::sync::MutexGuard<'_, Out> {
    out.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Decode one request line; a line that isn't a request gets a `REJECTED` answer (with
/// its id, when one can be read).
fn decode(line: &str) -> Result<Request, Box<Response>> {
    serde_json::from_str(line).map_err(|e| {
        let id = serde_json::from_str::<serde_json::Value>(line).ok().and_then(|v| v.get("id")?.as_str().map(String::from));
        Box::new(Response::error(id, ProtocolError::invalid_request(e)))
    })
}

pub fn serve() -> ExitCode {
    let host = Arc::new(Host::new());
    let out = Arc::new(Mutex::new(Out { stdout: std::io::stdout(), last_sent: None }));
    let done = Arc::new(AtomicBool::new(false));

    // Events: the state changed (scans ran, the debugger stopped, a scan failed).
    let events = {
        let (host, out, done) = (Arc::clone(&host), Arc::clone(&out), Arc::clone(&done));
        thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                thread::sleep(EVENT_INTERVAL);
                let mut out = lock(&out);
                let state = host.state();
                if out.last_sent.as_ref() != Some(&state) {
                    if out.send(&RuntimeMessage::StateUpdate { state: state.clone() }).is_err() {
                        return; // the client is gone; stdin will end too
                    }
                    out.last_sent = Some(state);
                }
            }
        })
    };

    eprintln!("[runtime] serving the protocol on stdin/stdout");
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let mut out = lock(&out);
        let response = match decode(&line) {
            Ok(Request { id, command }) => match host.execute(command) {
                Ok(state) => {
                    out.last_sent = Some(state.clone());
                    Response::ok(id, state)
                }
                Err(error) => Response::error(Some(id), error),
            },
            Err(rejected) => *rejected,
        };
        if out.send(&RuntimeMessage::Response(response)).is_err() {
            break;
        }
    }

    // The client closed the connection: stop the PLC (outputs to their safe state), exit.
    done.store(true, Ordering::Relaxed);
    let _ = host.execute(Command::Stop);
    let _ = events.join();
    eprintln!("[runtime] client disconnected, STOPPED");
    ExitCode::SUCCESS
}
