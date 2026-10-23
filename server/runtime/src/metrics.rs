//! Live numbers about the runtime for monitoring (Prometheus, Grafana): served as text at
//! `http://<listen address>/metrics`. Plain atomic counters, updated where things happen.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{SystemTime, UNIX_EPOCH};

pub static SCANS: AtomicU64 = AtomicU64::new(0);
/// How long the last scan's work took, and the longest since start (microseconds).
pub static SCAN_US: AtomicU64 = AtomicU64::new(0);
pub static SCAN_MAX_US: AtomicU64 = AtomicU64::new(0);
/// Time between the starts of the last two scans (microseconds); its gap to the
/// configured scan time is the jitter.
pub static SCAN_PERIOD_US: AtomicU64 = AtomicU64::new(0);
pub static SCAN_TIME_MS: AtomicU64 = AtomicU64::new(0);
pub static CLIENTS: AtomicU64 = AtomicU64::new(0);
pub static SCADA_CONNECTED: AtomicU64 = AtomicU64::new(0);
pub static SCADA_RECONNECTS: AtomicU64 = AtomicU64::new(0);
pub static SCADA_MESSAGES: AtomicU64 = AtomicU64::new(0);
pub static SCADA_VALUES: AtomicU64 = AtomicU64::new(0);
pub static SCADA_UNKNOWN: AtomicU64 = AtomicU64::new(0);
/// How long the last SCADA message took to read and apply (microseconds).
pub static SCADA_PROCESS_US: AtomicU64 = AtomicU64::new(0);
/// When the last SCADA message arrived (Unix time, milliseconds); 0 = never.
pub static SCADA_LAST_MS: AtomicU64 = AtomicU64::new(0);

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn record_scan(work_us: u64, period_us: u64) {
    SCANS.fetch_add(1, Relaxed);
    SCAN_US.store(work_us, Relaxed);
    SCAN_MAX_US.fetch_max(work_us, Relaxed);
    SCAN_PERIOD_US.store(period_us, Relaxed);
}

/// Every metric in Prometheus' text format.
pub fn render(running: bool) -> String {
    let get = |m: &AtomicU64| m.load(Relaxed);
    let secs = |us: u64| us as f64 / 1e6;
    let last = get(&SCADA_LAST_MS);
    let age = if last == 0 { -1.0 } else { now_ms().saturating_sub(last) as f64 / 1000.0 };
    let rows: [(&str, &str, &str, f64); 15] = [
        ("myplc_running", "gauge", "1 while the PLC is RUNNING", f64::from(u8::from(running))),
        ("myplc_scans_total", "counter", "PLC scans completed", get(&SCANS) as f64),
        ("myplc_scan_duration_seconds", "gauge", "Time the last scan's work took", secs(get(&SCAN_US))),
        ("myplc_scan_duration_max_seconds", "gauge", "Longest scan since start", secs(get(&SCAN_MAX_US))),
        ("myplc_scan_period_seconds", "gauge", "Time between the starts of the last two scans", secs(get(&SCAN_PERIOD_US))),
        ("myplc_scan_time_setting_seconds", "gauge", "Configured scan time", get(&SCAN_TIME_MS) as f64 / 1000.0),
        ("myplc_ide_clients", "gauge", "IDEs connected", get(&CLIENTS) as f64),
        ("myplc_scada_connected", "gauge", "1 while connected to the SCADA WebSocket", get(&SCADA_CONNECTED) as f64),
        ("myplc_scada_reconnects_total", "counter", "SCADA connections lost", get(&SCADA_RECONNECTS) as f64),
        ("myplc_scada_messages_total", "counter", "SCADA messages received", get(&SCADA_MESSAGES) as f64),
        ("myplc_scada_values_total", "counter", "Tag values applied", get(&SCADA_VALUES) as f64),
        ("myplc_scada_unknown_ids_total", "counter", "Tag IDs not in st_tags", get(&SCADA_UNKNOWN) as f64),
        ("myplc_scada_process_seconds", "gauge", "Time to read and apply the last SCADA message", secs(get(&SCADA_PROCESS_US))),
        ("myplc_scada_last_message_age_seconds", "gauge", "Seconds since the last SCADA message (-1 = none yet)", age),
        ("myplc_up", "gauge", "Always 1: the runtime answers", 1.0),
    ];
    rows.iter().map(|(name, kind, help, value)| format!("# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}\n")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_prometheus_text() {
        super::record_scan(1500, 100_000);
        let text = super::render(true);
        assert!(text.contains("# TYPE myplc_scans_total counter\nmyplc_scans_total "));
        assert!(text.contains("myplc_running 1\n"));
        assert!(text.contains("myplc_scan_period_seconds 0.1\n"));
    }
}
