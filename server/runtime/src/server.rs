//! `myplc-runtime --listen <addr>`: the PLC runtime as a network server, meant to run 24×7
//! (e.g. as a service on an on-prem machine). IDEs connect over a WebSocket at
//! `ws://<addr>` and speak the MyPLC protocol (`myplc_core::protocol`), one JSON message
//! per text frame: requests in; responses (same id) and `STATE_UPDATE` events out.
//!
//! - Any number of clients; they all control and watch the same PLC.
//! - A client leaving does not stop the PLC.
//! - The loaded project and the RUN/STOP mode are saved in the data folder, so after a
//!   restart (crash, reboot) the same project is loaded and, if it was running, runs again.
//! - With a token (`MYPLC_TOKEN`), clients must connect to `ws://<addr>/?token=…`, and
//!   `GET /metrics` must carry the same `?token=…`.
//! - Plain HTTP requests (hosting platforms' health checks) get `200 OK`, not a refusal;
//!   `GET /metrics` gets the runtime's live numbers for Prometheus (see `metrics`).

use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request as HttpRequest, Response as HttpResponse};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Error as WsError, Message, WebSocket};

use crate::host::Host;
use crate::metrics;
use myplc_core::protocol::{Command, ErrorCode, ProtocolError, Request, Response, ResponseData, RuntimeMessage, RuntimeState, RuntimeStatus};
use myplc_core::ENGINE_STACK_BYTES;

/// How long a connection waits for a request before checking the state for an update.
const EVENT_INTERVAL: Duration = Duration::from_millis(50);
/// The file in the data folder that remembers the project and the RUN/STOP mode.
const SAVED_FILE: &str = "runtime-state.json";
/// R8: refuse beyond this many simultaneous connections, so a flood can't exhaust threads.
const MAX_CONNECTIONS: usize = 128;
/// R8: the largest WebSocket message accepted, so one huge frame can't exhaust memory.
const MAX_MESSAGE_BYTES: usize = 1 << 20; // 1 MiB
/// Connections currently being served (for the `MAX_CONNECTIONS` cap).
static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
/// R9: within this window a source host may make at most `TOKEN_FAIL_MAX` wrong-token
/// attempts; beyond that it's refused until the window passes.
const TOKEN_FAIL_WINDOW: Duration = Duration::from_secs(60);
const TOKEN_FAIL_MAX: u32 = 5;
/// R9: pause after each wrong token, so sequential guessing is slow even below the cap.
const TOKEN_FAIL_DELAY: Duration = Duration::from_secs(1);

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
    if command == Command::GetTags {
        let response = match crate::tags::load() {
            Ok(tags) => Response { id: Some(id), result: Ok(ResponseData { state: host.state(), tags: Some(tags) }) },
            Err(message) => Response::error(Some(id), ProtocolError::new(ErrorCode::Unavailable, message)),
        };
        return (response, None);
    }
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

/// Compare a supplied token with the expected one in constant time: no early exit on the
/// first differing byte, so the time taken doesn't leak how much of a guess was right.
/// (Length can still differ; the token is a fixed secret, so that's acceptable.)
fn token_eq(got: Option<&str>, expected: &str) -> bool {
    let Some(got) = got else { return false };
    let (a, b) = (got.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// R9: wrong-token attempts per source host, as (count, window start). ponytail: entries are
/// kept per host — a tiny set on a PLC backend — so there's no eviction.
static TOKEN_FAILS: LazyLock<Mutex<HashMap<String, (u32, Instant)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The source host of a `peer` address ("ip:port" → "ip"): the rate-limit key.
fn host_of(peer: &str) -> &str {
    peer.rsplit_once(':').map_or(peer, |(host, _)| host)
}

/// Record one wrong-token attempt from `ip` at `now`; the count resets once the window passes.
fn note_token_fail(fails: &mut HashMap<String, (u32, Instant)>, ip: &str, now: Instant) {
    let entry = fails.entry(ip.to_string()).or_insert((0, now));
    if now.duration_since(entry.1) > TOKEN_FAIL_WINDOW {
        *entry = (0, now);
    }
    entry.0 += 1;
}

/// Whether `ip` is over the wrong-token cap within the current window.
fn token_locked(fails: &HashMap<String, (u32, Instant)>, ip: &str, now: Instant) -> bool {
    matches!(fails.get(ip), Some(&(count, start)) if count >= TOKEN_FAIL_MAX && now.duration_since(start) <= TOKEN_FAIL_WINDOW)
}

fn send(ws: &mut WebSocket<TcpStream>, message: &RuntimeMessage) -> Result<(), WsError> {
    ws.send(Message::text(serde_json::to_string(message).expect("protocol messages serialize")))
}

/// What a new connection wants, from its first bytes (left unread for the handshake).
enum Visitor {
    /// A WebSocket upgrade: an IDE.
    WebSocket,
    /// A plain HTTP request, e.g. a health check; `head` for a HEAD request, `metrics`
    /// for `/metrics`, `token` the `?token=` query parameter (used to guard `/metrics`).
    Http { head: bool, metrics: bool, token: Option<String> },
    /// Nothing usable (a port scan, or a client that went away).
    Nothing,
}

fn visitor(stream: &TcpStream) -> Visitor {
    let deadline = Instant::now() + Duration::from_secs(5);
    if stream.set_read_timeout(Some(Duration::from_millis(200))).is_err() {
        return Visitor::Nothing;
    }
    let mut buf = [0u8; 8192];
    loop {
        match stream.peek(&mut buf) {
            Ok(0) => return Visitor::Nothing,
            Ok(n) => {
                let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                // The request line and headers are complete (or too long to matter).
                if request.contains("\r\n\r\n") || n == buf.len() {
                    let upgrade = request.lines().any(|line| line.starts_with("upgrade:") && line.contains("websocket"));
                    let metrics = request.split(' ').nth(1).is_some_and(|path| path.starts_with("/metrics"));
                    // Token from the original (case-preserving) bytes, matched raw like `token_of`.
                    let token = String::from_utf8_lossy(&buf[..n])
                        .lines()
                        .next()
                        .and_then(|line| line.split(' ').nth(1))
                        .and_then(|path| path.split_once('?'))
                        .and_then(|(_, query)| query.split('&').find_map(|p| p.strip_prefix("token=")))
                        .map(str::to_string);
                    return if upgrade { Visitor::WebSocket } else { Visitor::Http { head: request.starts_with("head "), metrics, token } };
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Visitor::Nothing,
        }
        if Instant::now() > deadline {
            return Visitor::Nothing;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// A plain HTTP reply (`status` is e.g. `200 OK` or `401 Unauthorized`), then close.
fn answer_http(mut stream: TcpStream, head: bool, status: &str, body: String) {
    let mut request = [0u8; 8192];
    let _ = stream.read(&mut request); // consume it, so closing doesn't reset the connection
    let length = body.len();
    let body = if head { "" } else { &body };
    // nosniff: these endpoints return plain text; stop a browser from guessing another type.
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}");
    let _ = stream.write_all(response.as_bytes());
}

/// One client: answer its requests and push it every state change, until it leaves.
#[allow(clippy::result_large_err)] // the handshake callback's error type is tungstenite's
fn serve_client(stream: TcpStream, host: &Host, saved: &Saved, token: Option<&str>) {
    let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
    match visitor(&stream) {
        Visitor::WebSocket => {}
        Visitor::Http { head, metrics: false, .. } => return answer_http(stream, head, "200 OK", "ok
".into()),
        Visitor::Http { head, metrics: true, token: query_token } => {
            // R4: /metrics can expose performance details; when a token is set, require it here too.
            if let Some(expected) = token {
                if !token_eq(query_token.as_deref(), expected) {
                    return answer_http(stream, head, "401 Unauthorized", "A valid token is required.
".into());
                }
            }
            let running = host.state().status == RuntimeStatus::Running;
            return answer_http(stream, head, "200 OK", metrics::render(running));
        }
        Visitor::Nothing => return,
    }
    if stream.set_read_timeout(None).is_err() {
        return;
    }
    // R9: rate-limit token guesses by source host.
    let source = host_of(&peer).to_string();
    if token.is_some() && token_locked(&TOKEN_FAILS.lock().unwrap(), &source, Instant::now()) {
        eprintln!("[runtime] too many bad tokens from {source}; refusing for now");
        return; // connection dropped → closed
    }
    let check = |request: &HttpRequest, response: HttpResponse| -> Result<HttpResponse, ErrorResponse> {
        match token {
            Some(expected) if !token_eq(token_of(request), expected) => {
                // R9: count this wrong attempt, then pause to slow brute force.
                note_token_fail(&mut TOKEN_FAILS.lock().unwrap(), &source, Instant::now());
                thread::sleep(TOKEN_FAIL_DELAY);
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
    // R8: cap message and frame size so one huge message can't exhaust memory.
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let mut ws = match tungstenite::accept_hdr_with_config(stream, check, Some(config)) {
        Ok(ws) => ws,
        Err(e) => return eprintln!("[runtime] refused {peer}: {e}"),
    };
    if ws.get_ref().set_read_timeout(Some(EVENT_INTERVAL)).is_err() {
        return;
    }
    eprintln!("[runtime] client connected: {peer}");
    metrics::CLIENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    metrics::CLIENTS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
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
    // Live tag values: pushed by the SCADA over a WebSocket if MYPLC_SCADA_URL is set,
    // otherwise read from the tag database's data_log.
    if let Some(url) = crate::tags::scada_url() {
        let host = Arc::clone(&host);
        thread::spawn(move || crate::tags::listen(&url, host));
    } else if crate::tags::configured() {
        let host = Arc::clone(&host);
        thread::spawn(move || crate::tags::poll(host));
    }
    let saved = Arc::new(Saved { dir: config.data_dir.clone() });
    saved.restore(&host);
    let token: Option<Arc<str>> = config.token.map(Into::into);
    eprintln!(
        "[runtime] listening on ws://{}",
        config.listen,
    );
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // R8: cap simultaneous connections so a flood can't exhaust threads or memory.
        // ponytail: load-then-add may overshoot by a few under a burst; harmless.
        if CONNECTIONS.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            eprintln!("[runtime] too many connections ({MAX_CONNECTIONS}); refusing one");
            continue; // stream dropped here → connection closed
        }
        CONNECTIONS.fetch_add(1, Ordering::Relaxed);
        let (host, saved, token) = (Arc::clone(&host), Arc::clone(&saved), token.clone());
        // Commands like STEP run scans on the client's thread: give it the engine's stack.
        let spawned = thread::Builder::new()
            .name("plc-client".into())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn(move || {
                serve_client(stream, &host, &saved, token.as_deref());
                CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
            });
        if let Err(e) = spawned {
            CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
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

#[cfg(test)]
mod r9_tests {
    use super::*;

    #[test]
    fn locks_out_after_too_many_wrong_tokens() {
        let mut fails = HashMap::new();
        let t0 = Instant::now();
        assert!(!token_locked(&fails, "1.2.3.4", t0));
        for _ in 0..TOKEN_FAIL_MAX {
            note_token_fail(&mut fails, "1.2.3.4", t0);
        }
        assert!(token_locked(&fails, "1.2.3.4", t0));
        // A different host is unaffected.
        assert!(!token_locked(&fails, "5.6.7.8", t0));
        // Once the window passes, the next attempt starts a fresh count (not yet locked).
        let later = t0 + TOKEN_FAIL_WINDOW + Duration::from_secs(1);
        note_token_fail(&mut fails, "1.2.3.4", later);
        assert!(!token_locked(&fails, "1.2.3.4", later));
    }

    #[test]
    fn host_of_strips_the_port() {
        assert_eq!(host_of("127.0.0.1:54321"), "127.0.0.1");
        assert_eq!(host_of("[::1]:54321"), "[::1]");
    }

    #[test]
    fn token_eq_matches_only_the_right_token() {
        assert!(token_eq(Some("secret"), "secret"));
        assert!(!token_eq(Some("secre"), "secret")); // shorter
        assert!(!token_eq(Some("secrets"), "secret")); // longer
        assert!(!token_eq(Some("Secret"), "secret")); // wrong case
        assert!(!token_eq(None, "secret")); // missing
    }
}
