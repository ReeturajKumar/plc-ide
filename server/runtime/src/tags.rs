//! The SCADA tag database (PostgreSQL): the tag list from `st_tags` (see
//! `server/sql/st_tags.sql`; details from `scada_master_tags`), and their live values from
//! `data_log` (`poll`).
//!
//! Connection settings come from the environment, so changing them needs no code change:
//! `MYPLC_DB_HOST` (default localhost), `MYPLC_DB_PORT` (5432), `MYPLC_DB_NAME`,
//! `MYPLC_DB_USER`, `MYPLC_DB_PASSWORD`. A `.env` file in the working folder is read at
//! start (see `load_env_file`); real environment variables win over it.

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
            "SELECT st.tag_name, coalesce(s.description, ''), coalesce(s.data_type, ''), coalesce(s.engineering_unit, ''),
                    coalesce(s.device_name, ''), coalesce(s.protocol_type, ''), coalesce(s.protocol_address, ''),
                    coalesce(s.scale_factor, 1)::float8, coalesce(s.scale_offset, 0)::float8, coalesce(st.tag_class, '')
             FROM st_tags st
             JOIN tagmast t ON t.id = st.id
             LEFT JOIN scada_master_tags s ON s.source_tag_id = t.uuid::text
             WHERE s.is_enabled IS NOT FALSE
             ORDER BY st.id",
            &[],
        )
        .map_err(|e| format!("Can't read the tags from st_tags (create it with server/sql/st_tags.sql): {e}"))?;
    Ok(rows
        .iter()
        .map(|row| {
            let scada_type: String = row.get(2);
            Tag {
                name: row.get(0),
                description: row.get(1),
                // Numbers are engineering values after scaling (×0.1 etc.), so REAL.
                st_type: if scada_type.eq_ignore_ascii_case("boolean") { DataType::Bool } else { DataType::Real },
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
