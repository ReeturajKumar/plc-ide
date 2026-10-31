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
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::host::Host;
use crate::metrics;

use myplc_core::compiler::ast::DataType;
use myplc_core::compiler::lexer::{tokenize, TokenKind};
use myplc_core::compiler::project::ProgramSource;
use myplc_core::protocol::Tag;
use myplc_core::VALID_SUFFIX;
use postgres::{Client, Config, NoTls};

/// How often the SCADA listener may re-read tag names when an unknown id arrives (a tag a
/// program just started using, since `st_tags` is usage-driven).
const NAME_REFRESH: Duration = Duration::from_secs(3);

/// How often the backend writes its live tag values to `st_tags.last_value` (a durable
/// snapshot, so a restart can seed the last-known values instead of starting from zero).
const SNAPSHOT_EVERY: Duration = Duration::from_secs(5);

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

/// One SCADA message → (tag name, value) for the tags known in `names`; the IDs it didn't
/// know. Two formats: `{"type":"st_tags","tags":[{"tag_id","value"}]}` (or the older
/// `"type":"ScriptTagValues"`)
/// and a screen's `{"screen_id":23,"data":[{"TagID","Value"}]}`. Other messages give nothing.
// ponytail: values are taken as engineering values; apply scale_factor here if the SCADA
// turns out to send raw register values.
fn tag_values(text: &str, names: &HashMap<String, String>) -> (Vec<(String, f64)>, Vec<String>) {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(text) else { return Default::default() };
    let (tags, id_key, value_key) = if message["type"] == "st_tags" || message["type"] == "ScriptTagValues" {
        (&message["tags"], "tag_id", "value")
    } else if !message["screen_id"].is_null() {
        (&message["data"], "TagID", "Value")
    } else {
        return Default::default();
    };
    let (mut values, mut unknown) = (Vec::new(), Vec::new());
    for tag in tags.as_array().into_iter().flatten() {
        let id = tag[id_key].as_str().unwrap_or_default().to_lowercase();
        // R3: never apply a non-finite value (NaN, ±inf, or a number that overflows f64); a
        // bad or fake SCADA message must not poison a tag. Such values fall through to `unknown`.
        let value = tag[value_key]
            .as_f64()
            .or_else(|| tag[value_key].as_bool().map(f64::from))
            .filter(|v| v.is_finite());
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
        let mut names = match names_by_id() {
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
                metrics::SCADA_CONNECTED.store(1, Relaxed);
                let mut warned = false;
                let mut last_refresh = std::time::Instant::now();
                loop {
                    match ws.read() {
                        Ok(tungstenite::Message::Text(text)) => {
                            let start = std::time::Instant::now();
                            let (mut values, mut unknown) = tag_values(&text, &names);
                            // st_tags is usage-driven: an unknown id may be a tag a program just
                            // started using. Refresh the names (at most every NAME_REFRESH) and
                            // re-resolve this message so the new tag's value applies right away.
                            if !unknown.is_empty() && last_refresh.elapsed() >= NAME_REFRESH {
                                if let Ok(fresh) = names_by_id() {
                                    names = fresh;
                                    let (v, u) = tag_values(&text, &names);
                                    values = v;
                                    unknown = u;
                                    warned = false;
                                }
                                last_refresh = std::time::Instant::now();
                            }
                            metrics::SCADA_MESSAGES.fetch_add(1, Relaxed);
                            metrics::SCADA_LAST_MS.store(metrics::now_ms(), Relaxed);
                            metrics::SCADA_VALUES.fetch_add(values.len() as u64, Relaxed);
                            metrics::SCADA_UNKNOWN.fetch_add(unknown.len() as u64, Relaxed);
                            if !unknown.is_empty() && !warned {
                                eprintln!("[scada] tag IDs not in st_tags (ignored): {}", unknown.join(", "));
                                warned = true;
                            }
                            if !values.is_empty() {
                                host.set_tag_values(values);
                            }
                            metrics::SCADA_PROCESS_US.store(start.elapsed().as_micros() as u64, Relaxed);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            metrics::SCADA_CONNECTED.store(0, Relaxed);
                            metrics::SCADA_RECONNECTS.fetch_add(1, Relaxed);
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

/// Every available tag, from `tagmast` (the SCADA's full catalog), in its order. These are
/// what a program may use; the ones actually written are tracked separately in `st_tags`.
pub fn load() -> Result<Vec<Tag>, String> {
    let mut client = connect()?;
    let rows = client
        .query(
            "SELECT t.tag_name, coalesce(s.description, ''), coalesce(c.tag, t.data_type, ''), coalesce(t.unit, s.engineering_unit, ''),
                    coalesce(s.device_name, ''), coalesce(s.protocol_type, ''), coalesce(s.protocol_address, ''),
                    coalesce(s.scale_factor, 1)::float8, coalesce(s.scale_offset, 0)::float8, coalesce(t.tag_type, '')
             FROM tagmast t
             LEFT JOIN scada_master_tags s ON s.tag_name = t.tag_name
             LEFT JOIN control_table c ON c.code = 5 AND c.id::text = t.data_type
             WHERE t.tag_name IS NOT NULL AND s.is_enabled IS NOT FALSE
             ORDER BY t.id",
            &[],
        )
        .map_err(|e| format!("Can't read the tags from tagmast: {e}"))?;
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

/// Count each available tag's occurrences across the project's sources. Only ST identifiers
/// that name a tag are counted; the lexer drops comments and strings, so those don't inflate
/// it. Returns (canonical tag name, count), case-insensitively like every ST name.
fn tag_usage(programs: &[ProgramSource], available: &[String]) -> Vec<(String, i32)> {
    let canonical: HashMap<String, &String> = available.iter().map(|n| (n.to_uppercase(), n)).collect();
    let mut counts: HashMap<String, i32> = HashMap::new();
    for program in programs {
        for token in tokenize(&program.source) {
            if let TokenKind::Identifier(name) = token.kind {
                let upper = name.to_uppercase();
                // `X_VALID` is X's live-value flag: reading it uses X (so X is tracked and streamed).
                let base = upper.strip_suffix(VALID_SUFFIX).filter(|b| canonical.contains_key(*b));
                if let Some(canon) = canonical.get(base.unwrap_or(&upper)) {
                    *counts.entry((*canon).clone()).or_insert(0) += 1;
                }
            }
        }
    }
    counts.into_iter().collect()
}

/// Record which SCADA tags `project_id` uses (and how many times) in `st_tags`: set this
/// project's slice, recompute each tag's total, and drop any tag whose total reaches zero.
/// Writes **only** `st_tags`. A database error is returned so the caller can log it without
/// failing the load.
pub fn reconcile(project_id: &str, programs: &[ProgramSource], available: &[String]) -> Result<(), String> {
    let usage = tag_usage(programs, available);
    let used_names: Vec<String> = usage.iter().map(|(name, _)| name.clone()).collect();
    let mut client = connect()?;
    let mut tx = client.transaction().map_err(|e| format!("st_tags update: {e}"))?;
    for (name, count) in &usage {
        tx.execute(
            "INSERT INTO st_tags (id, tagid, tag_name, tag_class, data_type, usage_by_project, usage_count)
             SELECT t.id, t.tag_id, t.tag_name, t.tag_type, t.data_type, jsonb_build_object($1::text, $3::int), $3::int
             FROM tagmast t WHERE t.tag_name = $2
             ON CONFLICT (id) DO UPDATE SET
                 usage_by_project = st_tags.usage_by_project || jsonb_build_object($1::text, $3::int),
                 usage_count = coalesce((SELECT sum(v::int) FROM jsonb_each_text(st_tags.usage_by_project || jsonb_build_object($1::text, $3::int)) e(k, v)), 0)",
            &[&project_id, name, count],
        )
        .map_err(|e| format!("st_tags upsert: {e}"))?;
    }
    // Tags this project used before but no longer does: drop its slice.
    tx.execute(
        "UPDATE st_tags SET
             usage_by_project = usage_by_project - $1::text,
             usage_count = coalesce((SELECT sum(v::int) FROM jsonb_each_text(usage_by_project - $1::text) e(k, v)), 0)
         WHERE jsonb_exists(usage_by_project, $1) AND NOT (tag_name = ANY($2))",
        &[&project_id, &used_names],
    )
    .map_err(|e| format!("st_tags prune: {e}"))?;
    tx.execute("DELETE FROM st_tags WHERE usage_count <= 0", &[]).map_err(|e| format!("st_tags cleanup: {e}"))?;
    tx.commit().map_err(|e| format!("st_tags commit: {e}"))?;
    Ok(())
}

/// Seed the host with each tag's last snapshotted value (`st_tags.last_value`), so a restart
/// starts from the last-known values instead of zero. Called once at startup, before the
/// saved project is restored (so the restored program sees these values). Best-effort.
pub fn seed(host: &Host) {
    let mut client = match connect() {
        Ok(client) => client,
        Err(_) => return, // no database: nothing to seed from
    };
    match client.query("SELECT tag_name, last_value FROM st_tags WHERE last_value IS NOT NULL", &[]) {
        Ok(rows) => {
            let values: Vec<(String, f64)> = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
            if !values.is_empty() {
                eprintln!("[tags] seeded {} tag values from the last snapshot (their _VALID flags stay FALSE until live)", values.len());
                host.seed_tag_values(values);
            }
        }
        Err(e) => eprintln!("[tags] couldn't seed tag values: {e}"),
    }
}

/// Forever: every `SNAPSHOT_EVERY`, write the host's current tag values to `st_tags.last_value`
/// in one batch, so they survive a restart. Writes only `st_tags`.
pub fn snapshot(host: Arc<Host>) {
    loop {
        thread::sleep(SNAPSHOT_EVERY);
        let values = host.tag_values_snapshot();
        if values.is_empty() {
            continue;
        }
        let names: Vec<String> = values.iter().map(|(n, _)| n.clone()).collect();
        let nums: Vec<f64> = values.iter().map(|(_, v)| *v).collect();
        let Ok(mut client) = connect() else { continue };
        // One statement updates every used tag's row; it only touches st_tags.
        let done = client.execute(
            "UPDATE st_tags s SET last_value = v.val, last_value_at = now()
             FROM (SELECT unnest($1::text[]) AS name, unnest($2::float8[]) AS val) v
             WHERE s.tag_name = v.name",
            &[&names, &nums],
        );
        if let Err(e) = done {
            eprintln!("[tags] couldn't snapshot tag values: {e}");
        }
    }
}

/// A project was deleted: remove its slice from every tag's counts, dropping tags that reach
/// zero. Writes only `st_tags`.
pub fn release(project_id: &str) -> Result<(), String> {
    let mut client = connect()?;
    client
        .execute(
            "UPDATE st_tags SET
                 usage_by_project = usage_by_project - $1::text,
                 usage_count = coalesce((SELECT sum(v::int) FROM jsonb_each_text(usage_by_project - $1::text) e(k, v)), 0)
             WHERE jsonb_exists(usage_by_project, $1)",
            &[&project_id],
        )
        .map_err(|e| format!("st_tags release: {e}"))?;
    client.execute("DELETE FROM st_tags WHERE usage_count <= 0", &[]).map_err(|e| format!("st_tags release cleanup: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_usage_counts_occurrences_ignoring_comments() {
        let available = vec!["TANK1_LEVEL".to_string(), "PUMP1".to_string()];
        let src = "x := TANK1_LEVEL; y := tank1_level + PUMP1; z := TANK1_LEVEL_VALID; (* TANK1_LEVEL in a comment *)";
        let programs = vec![ProgramSource { name: "p".into(), source: src.into() }];
        let counts: HashMap<_, _> = tag_usage(&programs, &available).into_iter().collect();
        assert_eq!(counts.get("TANK1_LEVEL"), Some(&3)); // case-insensitive; _VALID counts; the comment doesn't
        assert_eq!(counts.get("PUMP1"), Some(&1));
        assert_eq!(counts.get("UNUSED"), None); // a name not in `available` is ignored
    }

    #[test]
    fn reads_script_and_screen_messages() {
        let names = HashMap::from([("abc".to_string(), "T1".to_string())]);
        let script = r#"{"type":"ScriptTagValues","script_id":"117","tags":[{"tag_id":"ABC","register_address":5,"value":1.5},{"tag_id":"zzz","value":2}]}"#;
        assert_eq!(tag_values(script, &names), (vec![("T1".to_string(), 1.5)], vec!["zzz".to_string()]));
        let st = r#"{"type":"st_tags","tags":[{"tag_id":"abc","value":true}]}"#;
        assert_eq!(tag_values(st, &names), (vec![("T1".to_string(), 1.0)], vec![]));
        let screen =r#"{"screen_name":"Screen 23","screen_id":23,"data":[{"TagID":"abc","RegAddress":633,"FunctionType":"HoldingRegister","Value":9407.000000}]}"#;
        assert_eq!(tag_values(screen, &names), (vec![("T1".to_string(), 9407.0)], vec![]));
        assert_eq!(tag_values(r#"{"hello":1}"#, &names), Default::default());
        // R3: a garbage/overflowing number can't poison a tag — serde_json rejects the whole
        // message, and the is_finite filter guards any non-finite value that reached us.
        let bad = r#"{"type":"st_tags","tags":[{"tag_id":"abc","value":1e400}]}"#;
        assert_eq!(tag_values(bad, &names), Default::default());
        // A wrong-typed value (text where a number is expected) is ignored, not applied.
        let wrong = r#"{"type":"st_tags","tags":[{"tag_id":"abc","value":"oops"}]}"#;
        assert_eq!(tag_values(wrong, &names), (vec![], vec!["abc".to_string()]));
    }
}
