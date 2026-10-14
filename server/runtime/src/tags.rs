//! The SCADA tag database (PostgreSQL): the tag list from `st_tags` (see
//! `server/sql/st_tags.sql`), the type names from `control_table` (group 5: 54 Binary,
//! 57 Float ABCD, …), and details from `scada_master_tags` where it has the tag. Live values come from the
//! SCADA's WebSocket if `MYPLC_SCADA_URL` is set (`listen`; `mock-scada/` stands in for
//! it), otherwise from `data_log` (`poll`).
//!
//! Connection settings come from the environment, so changing them needs no code change:
//! `MYPLC_DB_HOST` (default localhost), `MYPLC_DB_PORT` (5432), `MYPLC_DB_NAME`,
//! `MYPLC_DB_USER`, `MYPLC_DB_PASSWORD`. A `.env` file in the working folder is read at
//! start (see `load_env_file`); real environment variables win over it.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::host::Host;

use myplc_core::compiler::ast::DataType;
use myplc_core::protocol::Tag;
use postgres::{Client, Config, NoTls};

/// Set every `KEY=VALUE` of `.env` (if there is one) that isn't already set.
pub fn load_env_file() {
    let Ok(text) = fs::read_to_string(".env") else { return };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        if let Some((key, value)) = line.split_once('=') {
            if env::var_os(key.trim()).is_none() {
                env::set_var(key.trim(), value.trim());
            }
        }
    }
}

fn connect() -> Result<Client, String> {
    let setting = |key: &str| env::var(key).ok().filter(|v| !v.is_empty());
    let name = setting("MYPLC_DB_NAME").ok_or("No tag database configured: set MYPLC_DB_NAME (and the other MYPLC_DB_* settings) in server/.env")?;
    let host = setting("MYPLC_DB_HOST").unwrap_or_else(|| "localhost".into());
    let port = setting("MYPLC_DB_PORT").and_then(|p| p.parse().ok()).unwrap_or(5432);
    let mut config = Config::new();
    config.host(&host).port(port).dbname(&name).connect_timeout(Duration::from_secs(5));
    if let Some(user) = setting("MYPLC_DB_USER") {
        config.user(&user);
    }
    if let Some(password) = setting("MYPLC_DB_PASSWORD") {
        config.password(password);
    }
    config.connect(NoTls).map_err(|e| {
        // The top error is often vague ("invalid configuration"); its cause says what's wrong.
        let cause = std::error::Error::source(&e).map(|c| format!(": {c}")).unwrap_or_default();
        format!("Tag database not reachable ({name} on {host}:{port}): {e}{cause}. Check the MYPLC_DB_* settings in server/.env")
    })
}

/// Whether a tag database is configured at all.
pub fn configured() -> bool {
    env::var("MYPLC_DB_NAME").is_ok_and(|v| !v.is_empty())
}

/// Forever: every `MYPLC_DB_REFRESH_MS` (default 1000), give `host` each tag's latest
/// value from `data_log`. Survives the database going away: tags keep their last value
/// and it reconnects.
pub fn poll(host: Arc<Host>) {
    let every = Duration::from_millis(env::var("MYPLC_DB_REFRESH_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000).max(100));
    let mut client: Option<Client> = None;
    let mut last_error = String::new();
    loop {
        let result = match client.as_mut() {
            Some(c) => latest_values(c),
            None => connect().map(|c| {
                client = Some(c);
                Vec::new()
            }),
        };
        match result {
            Ok(values) => {
                if !last_error.is_empty() {
                    eprintln!("[tags] database reachable again");
                    last_error.clear();
                }
                if !values.is_empty() {
                    host.set_tag_values(values);
                }
            }
            Err(e) => {
                client = None;
                if e != last_error {
                    eprintln!("[tags] {e} (tags keep their last values; retrying)");
                    last_error = e;
                }
            }
        }
        thread::sleep(every);
    }
}

/// The SCADA's WebSocket (`MYPLC_SCADA_URL`, e.g. ws://127.0.0.1:6000), if set.
pub fn scada_url() -> Option<String> {
    env::var("MYPLC_SCADA_URL").ok().filter(|v| !v.is_empty())
}

/// Tag ID (`st_tags.tagid`) → tag name, for the SCADA's messages.
fn names_by_id() -> Result<HashMap<String, String>, String> {
    let rows = connect()?
        .query("SELECT tagid::text, tag_name FROM st_tags WHERE tagid IS NOT NULL", &[])
        .map_err(|e| format!("Can't read st_tags: {e}"))?;
    Ok(rows.iter().map(|row| (row.get::<_, String>(0).to_lowercase(), row.get(1))).collect())
}

/// One `ScriptTagValues` message → (tag name, value) for the tags known in `names`; the
/// IDs it didn't know. Other messages give nothing.
// ponytail: values are taken as engineering values; apply scale_factor here if the SCADA
// turns out to send raw register values.
fn tag_values(text: &str, names: &HashMap<String, String>) -> (Vec<(String, f64)>, Vec<String>) {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(text) else { return Default::default() };
    if message["type"] != "ScriptTagValues" {
        return Default::default();
    }
    let (mut values, mut unknown) = (Vec::new(), Vec::new());
    for tag in message["tags"].as_array().into_iter().flatten() {
        let id = tag["tag_id"].as_str().unwrap_or_default().to_lowercase();
        let value = tag["value"].as_f64().or_else(|| tag["value"].as_bool().map(f64::from));
        match (names.get(&id), value) {
            (Some(name), Some(value)) => values.push((name.clone(), value)),
            _ => unknown.push(id),
        }
    }
    (values, unknown)
}

/// Forever: receive the SCADA's tag values over its WebSocket at `url` and give them to
/// `host`. Reconnects (every 2 s) if the connection drops; tags keep their last values.
pub fn listen(url: &str, host: Arc<Host>) {
    let mut last_error = String::new();
    let mut report = |e: String| {
        if e != last_error {
            eprintln!("[scada] {e} (tags keep their last values; retrying)");
            last_error = e;
        }
    };
    loop {
        // Fresh names on every connection: tags may have been added meanwhile.
        let names = match names_by_id() {
            Ok(names) => names,
            Err(e) => {
                report(e);
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        match tungstenite::connect(url) {
            Ok((mut ws, _)) => {
                eprintln!("[scada] connected to {url} ({} tags known)", names.len());
                let mut warned = false;
                loop {
                    match ws.read() {
                        Ok(tungstenite::Message::Text(text)) => {
                            let (values, unknown) = tag_values(&text, &names);
                            if !unknown.is_empty() && !warned {
                                eprintln!("[scada] tag IDs not in st_tags (ignored): {}", unknown.join(", "));
                                warned = true;
                            }
                            if !values.is_empty() {
                                host.set_tag_values(values);
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            report(format!("connection to {url} lost: {e}"));
                            break;
                        }
                    }
                }
            }
            Err(e) => report(format!("can't connect to {url}: {e}")),
        }
        thread::sleep(Duration::from_secs(2));
    }
}

/// Each tag's newest value in `data_log`.
// ponytail: DISTINCT ON scans data_log; add an index on (tag_name, cdttime) when it grows.
fn latest_values(client: &mut Client) -> Result<Vec<(String, f64)>, String> {
    let rows = client
        .query(
            "SELECT DISTINCT ON (tag_name) tag_name, curr_value FROM data_log
             WHERE curr_value IS NOT NULL ORDER BY tag_name, cdttime DESC",
            &[],
        )
        .map_err(|e| format!("Can't read tag values from data_log: {e}"))?;
    Ok(rows.iter().map(|row| (row.get(0), row.get(1))).collect())
}

/// Every enabled tag, in the SCADA's order.
pub fn load() -> Result<Vec<Tag>, String> {
    let mut client = connect()?;
    let rows = client
        .query(
            "SELECT st.tag_name, coalesce(s.description, ''), coalesce(c.tag, st.data_type, ''), coalesce(t.unit, s.engineering_unit, ''),
                    coalesce(s.device_name, ''), coalesce(s.protocol_type, ''), coalesce(s.protocol_address, ''),
                    coalesce(s.scale_factor, 1)::float8, coalesce(s.scale_offset, 0)::float8, coalesce(st.tag_class, '')
             FROM st_tags st
             JOIN tagmast t ON t.id = st.id
             LEFT JOIN scada_master_tags s ON s.tag_name = st.tag_name
             LEFT JOIN control_table c ON c.code = 5 AND c.id::text = st.data_type
             WHERE st.tag_name IS NOT NULL AND s.is_enabled IS NOT FALSE
             ORDER BY st.id",
            &[],
        )
        .map_err(|e| format!("Can't read the tags from st_tags (create it with server/sql/st_tags.sql): {e}"))?;
    Ok(rows
        .iter()
        .map(|row| {
            // control_table's name for the type ("Binary", "Float ABCD", "Signed", …).
            let scada_type: String = row.get(2);
            let boolean = ["binary", "boolean"].iter().any(|b| scada_type.eq_ignore_ascii_case(b));
            Tag {
                name: row.get(0),
                description: row.get(1),
                // Numbers are engineering values after scaling (×0.1 etc.), so REAL.
                st_type: if boolean { DataType::Bool } else { DataType::Real },
                scada_type,
                unit: row.get(3),
                device: row.get(4),
                protocol: row.get(5),
                address: row.get(6),
                scale_factor: row.get(7),
                scale_offset: row.get(8),
                tag_class: row.get(9),
            }
        })
        .collect())
}
