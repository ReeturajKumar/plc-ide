//! mock-scada: stands in for the real SCADA while testing. It serves a WebSocket and sends
//! every connected client the tags' live values each second, in the SCADA's message format:
//!
//! ```json
//! { "type": "ScriptTagValues", "script_id": "117", "tag_count": 2,
//!   "tags": [ { "tag_id": "d623d030-…", "register_address": 40001, "value": 230.5 }, … ] }
//! ```
//!
//! The tag IDs are the real ones from `st_tags`, so the backend can match them to names.
//! Settings: the database from `server/.env` (MYPLC_DB_*), plus
//! `MOCK_SCADA_LISTEN` (default 127.0.0.1:6000) and `MOCK_SCADA_INTERVAL_MS` (default 1000).
//!
//! Run: `cd mock-scada && cargo run`

use std::env;
use std::fs;
use std::net::TcpListener;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How often the mock re-reads `st_tags` (which is usage-driven: it grows as programs compile).
const RELOAD_SECS: u64 = 3;

use postgres::{Config, NoTls};
use serde_json::json;
use tungstenite::{accept, Message};

/// One tag the mock sends.
struct Tag {
    id: String,
    register: i64,
    boolean: bool,
}

/// Read `KEY=VALUE` lines of `path` into the environment (variables already set win).
fn load_env(path: &str) {
    let Ok(text) = fs::read_to_string(path) else { return };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((key, value)) = line.split_once('=') {
            if env::var_os(key.trim()).is_none() {
                env::set_var(key.trim(), value.trim());
            }
        }
    }
}

fn setting(key: &str, default: &str) -> String {
    env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

/// The tags from `st_tags`: real IDs, the register (the digits of `protocol_address`, 0 if
/// `scada_master_tags` doesn't have the tag), Binary (control_table) or a number.
fn load_tags() -> Result<Vec<Tag>, String> {
    let mut config = Config::new();
    config
        .host(&setting("MYPLC_DB_HOST", "localhost"))
        .port(setting("MYPLC_DB_PORT", "5432").parse().unwrap_or(5432))
        .dbname(&setting("MYPLC_DB_NAME", ""))
        .user(&setting("MYPLC_DB_USER", "postgres"))
        .password(setting("MYPLC_DB_PASSWORD", ""))
        .connect_timeout(Duration::from_secs(5));
    let mut client = config.connect(NoTls).map_err(|e| format!("can't reach the tag database: {e} (check server/.env)"))?;
    let rows = client
        .query(
            "SELECT st.tagid, coalesce(nullif(regexp_replace(coalesce(s.protocol_address, ''), '\\D', '', 'g'), ''), '0')::int8,
                    coalesce(c.tag, '') ILIKE 'binary'
             FROM st_tags st
             LEFT JOIN scada_master_tags s ON s.tag_name = st.tag_name
             LEFT JOIN control_table c ON c.code = 5 AND c.id::text = st.data_type
             WHERE st.tagid IS NOT NULL ORDER BY st.id",
            &[],
        )
        .map_err(|e| format!("can't read st_tags: {e}"))?;
    Ok(rows.iter().map(|r| Tag { id: r.get(0), register: r.get(1), boolean: r.get(2) }).collect())
}

/// Tag `i`'s value `t` seconds in: numbers drift around their own base, Booleans toggle
/// every few seconds.
fn value(tag: &Tag, i: usize, t: f64) -> serde_json::Value {
    if tag.boolean {
        return json!(((t as usize) / (3 + i % 4)) % 2 == 1);
    }
    let base = 50.0 + ((i * 37) % 200) as f64;
    json!(((base + 10.0 * (t / 5.0 + i as f64).sin()) * 100.0).round() / 100.0)
}

fn message(tags: &[Tag], t: f64) -> String {
    let values: Vec<_> = tags
        .iter()
        .enumerate()
        .map(|(i, tag)| json!({ "tag_id": tag.id, "register_address": tag.register, "value": value(tag, i, t) }))
        .collect();
    json!({ "type": "ScriptTagValues", "script_id": "117", "tag_count": values.len(), "tags": values }).to_string()
}

fn main() -> ExitCode {
    load_env(".env");
    load_env("../server/.env");
    // st_tags is usage-driven: it may be empty and grows as programs are compiled. Start even
    // when empty; a reader thread below re-reads it so newly-used tags begin streaming without
    // a restart. Only an unreachable database is fatal here.
    let tags: Arc<Mutex<Arc<[Tag]>>> = match load_tags() {
        Ok(tags) => Arc::new(Mutex::new(tags.into())),
        Err(e) => {
            eprintln!("[mock-scada] {e}");
            return ExitCode::FAILURE;
        }
    };
    // On a host like Render, bind to 0.0.0.0 and the injected $PORT; otherwise localhost for dev.
    // MOCK_SCADA_LISTEN still wins when set explicitly.
    let address = env::var("MOCK_SCADA_LISTEN").ok().filter(|v| !v.is_empty())
        .or_else(|| env::var("PORT").ok().filter(|v| !v.is_empty()).map(|p| format!("0.0.0.0:{p}")))
        .unwrap_or_else(|| "127.0.0.1:6000".to_string());
    let every = Duration::from_millis(setting("MOCK_SCADA_INTERVAL_MS", "1000").parse().unwrap_or(1000));
    let listener = match TcpListener::bind(&address) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("[mock-scada] can't listen on {address}: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("[mock-scada] listening on ws://{address}, sending st_tags every {} ms (reloads every {RELOAD_SECS}s)", every.as_millis());
    // Re-read st_tags periodically so usage changes (tags added/removed by compiling) take effect.
    {
        let tags = Arc::clone(&tags);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(RELOAD_SECS));
            if let Ok(fresh) = load_tags() {
                *tags.lock().unwrap() = fresh.into();
            }
        });
    }
    let start = Instant::now();
    for stream in listener.incoming().flatten() {
        let tags = Arc::clone(&tags);
        thread::spawn(move || {
            let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
            let Ok(mut ws) = accept(stream) else { return };
            eprintln!("[mock-scada] client connected: {peer}");
            loop {
                let snapshot = tags.lock().unwrap().clone(); // Arc<[Tag]>: cheap, releases the lock
                if ws.send(Message::text(message(&snapshot, start.elapsed().as_secs_f64()))).is_err() {
                    break;
                }
                thread::sleep(every);
            }
            eprintln!("[mock-scada] client left: {peer}");
        });
    }
    ExitCode::SUCCESS
}
