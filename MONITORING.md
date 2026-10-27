# MyPLC Performance Monitoring – Manual

How to run MyPLC with live performance monitoring (Prometheus + Grafana), see the metrics and dashboard, and read the results.

---

## 1. How it works

```
SCADA (real or mock-scada) ──JSON──►  MyPLC backend (myplc-runtime)  ◄── IDE
                                          │
                                          │  http://127.0.0.1:5020/metrics
                                          ▼
                                   Prometheus (collects every 5 s, stores history)
                                          │
                                          ▼
                                   Grafana (dashboard, refreshes every 5 s)
```

| Part | What it does | Address |
|---|---|---|
| MyPLC backend | Runs the PLC and shows its live numbers | `http://127.0.0.1:5020/metrics` |
| Prometheus | Reads those numbers every 5 s and keeps a history | `http://localhost:9090` |
| Grafana | Shows them as a dashboard | `http://localhost:3000` |

---

## 2. One-time setup

### 2.1 Requirements

- Windows 10/11
- Rust (`cargo`), Node.js, PostgreSQL with the tag database (see `README.md`)
- `server/.env` filled in (copy `server/.env.example`). For testing with the mock SCADA:
  ```
  MYPLC_SCADA_URL=ws://127.0.0.1:6000
  ```

### 2.2 Install Prometheus

1. Go to https://prometheus.io/download, section **prometheus**, and download the file ending in `windows-amd64.zip`.
2. Right-click the zip, choose **Extract All…**, and set the destination to `C:\`.
3. Find the folder that contains `prometheus.exe`, e.g. `C:\prometheus-3.15.1.windows-amd64\`.
   (Below it's called `<PROMETHEUS>`.)

Nothing else is needed: the config file is already in this project, at `monitoring/prometheus.yml`.

### 2.3 Install Grafana

1. Go to https://grafana.com/grafana/download?platform=windows, choose Edition **OSS**, and download the **Windows Installer** (`.msi`).
2. Double-click the `.msi`, click **Next** on each screen, then **Install** (answer **Yes** if Windows asks for permission), then **Finish**.
3. Grafana installs as a Windows service and starts by itself, now and after every reboot. It can take about a minute after installing.
4. Open http://localhost:3000 and log in with `admin` / `admin`, then set a new password.

### 2.4 Connect Grafana to Prometheus

1. Left menu → **Connections** → **Data sources** → **Add data source** → **Prometheus**.
2. **Prometheus server URL**: `http://localhost:9090`
3. Scroll down → **Save & test**. You should see "Successfully queried the Prometheus API".
   (Prometheus must be running for this test: see step 3.4 below.)

### 2.5 Import the dashboard

1. Left menu → **Dashboards** → **New** (top right) → **Import**.
2. **Upload dashboard JSON file** → choose `monitoring/grafana-dashboard.json` from this project.
3. Click **Import**. The dashboard **MyPLC Runtime** opens.

Setup is done. From now on, only section 3 is needed.

---

## 3. Every time: start everything

Open a separate terminal for each step and keep it open. The order matters.

**3.1 PostgreSQL**: must be running (it normally starts with Windows).

**3.2 SCADA source**: either your real SCADA (its URL is set in `MYPLC_SCADA_URL` in `server/.env`) or the mock:
```bash
cd mock-scada
cargo run
```
The mock sends all tags every second at `ws://127.0.0.1:6000`.

**3.3 MyPLC backend**: use the **release** build when measuring performance (the debug build is much slower):
```bash
cd server
cargo run --release -p myplc-runtime -- --listen 127.0.0.1:5020
```
Expected log: `[scada] connected to ws://127.0.0.1:6000 (20017 tags known)`.
Check: http://127.0.0.1:5020/metrics shows lines starting with `myplc_`.

**3.4 Prometheus**:
```bash
<PROMETHEUS>\prometheus.exe --config.file=C:\Users\<you>\Desktop\my-plc-ide\monitoring\prometheus.yml
```
Expected log: `Server is ready to receive web requests.`
Check: http://localhost:9090/targets shows **myplc** as **UP** (green).
If Windows Firewall asks, click **Allow**.

**3.5 IDE**: open the IDE, load a program and press **Run**.
```bash
cd ui
npm run tauri dev
```

**3.6 Grafana**: open http://localhost:3000 → **Dashboards** → **MyPLC Runtime**.

---

## 4. The Grafana dashboard

It refreshes every 5 s and shows the last 15 minutes. Change the time range at the top right.

### 4.1 Status boxes (top row)

| Box | Good value | Meaning |
|---|---|---|
| PLC | RUNNING | Is the PLC scanning? |
| SCADA link | CONNECTED | Is the backend connected to the SCADA WebSocket? |
| IDE clients | 1 or more | Number of IDEs connected |
| Longest scan | far below the scan time (e.g. < 10 ms for 100 ms) | Slowest scan since the backend started |
| SCADA reconnects | 0 | Times the SCADA connection dropped |
| Unknown tag IDs | 0 | Tag IDs the SCADA sent that are not in `st_tags` |

### 4.2 Graphs

| Graph | What it shows | Healthy |
|---|---|---|
| Scan duration | Time each scan's work takes (last + max) | A small fraction of the scan time |
| Scan period vs setting | Real time between scans vs the configured scan time | Close to the setting (e.g. 100–101 ms) |
| Scan jitter | How far the period is off the setting | A few ms at most |
| Scans per second | Scan rate | About 1000 / scan time (≈ 10 for 100 ms) |
| SCADA messages and values per second | Messages received; tag values applied | Matches what the SCADA sends |
| SCADA latency | Time to apply one message; age of the last message | Processing well below the send interval; age below 2× the send interval |

### 4.3 Baseline (measured 22 Oct 2026, debug build, mock SCADA with 20,017 tags)

| Metric | Value |
|---|---|
| Scan duration | 0.1–0.2 ms |
| Longest scan | 3.88 ms |
| Scan period | 100–101 ms (setting 100 ms) |
| Jitter | 0.2–1.1 ms |
| Scans per second | ≈ 9.9 |
| SCADA values per second | ≈ 15–16K |
| SCADA processing per message | ≈ 150–250 ms (all 20,017 tags; the release build should be faster) |
| Reconnects / unknown IDs | 0 / 0 |

Use this table to compare new results.

---

## 5. Prometheus without Grafana

Open http://localhost:9090 → **Query**, paste a query, click **Execute**, and open the **Graph** tab (set the range to `5m`).

**Everything at once** (Table tab):
```
{job="myplc"}
```

**All graphs on one page**: open this link:
```
http://localhost:9090/query?g0.expr=myplc_running&g0.tab=graph&g0.range_input=5m&g1.expr=myplc_scan_duration_seconds+%2A+1000&g1.tab=graph&g1.range_input=5m&g2.expr=myplc_scan_period_seconds+%2A+1000&g2.tab=graph&g2.range_input=5m&g3.expr=abs%28myplc_scan_period_seconds+-+myplc_scan_time_setting_seconds%29+%2A+1000&g3.tab=graph&g3.range_input=5m&g4.expr=rate%28myplc_scans_total%5B1m%5D%29&g4.tab=graph&g4.range_input=5m&g5.expr=myplc_scada_connected&g5.tab=graph&g5.range_input=5m&g6.expr=rate%28myplc_scada_messages_total%5B1m%5D%29&g6.tab=graph&g6.range_input=5m&g7.expr=rate%28myplc_scada_values_total%5B1m%5D%29&g7.tab=graph&g7.range_input=5m&g8.expr=myplc_scada_process_seconds+%2A+1000&g8.tab=graph&g8.range_input=5m&g9.expr=myplc_scada_last_message_age_seconds&g9.tab=graph&g9.range_input=5m&g10.expr=myplc_ide_clients&g10.tab=graph&g10.range_input=5m
```

**Useful queries:**

| Query | Shows |
|---|---|
| `myplc_running` | 1 = RUNNING, 0 = STOPPED |
| `myplc_scan_duration_seconds * 1000` | Scan time (ms) |
| `myplc_scan_duration_max_seconds * 1000` | Longest scan (ms) |
| `myplc_scan_period_seconds * 1000` | Time between scans (ms) |
| `abs(myplc_scan_period_seconds - myplc_scan_time_setting_seconds) * 1000` | Jitter (ms) |
| `rate(myplc_scans_total[1m])` | Scans per second |
| `myplc_scada_connected` | 1 = connected, 0 = down |
| `rate(myplc_scada_messages_total[1m])` | SCADA messages per second |
| `rate(myplc_scada_values_total[1m])` | Tag values per second |
| `myplc_scada_process_seconds * 1000` | SCADA processing per message (ms) |
| `myplc_scada_last_message_age_seconds` | Seconds since the last SCADA message |
| `myplc_scada_unknown_ids_total` | Unknown tag IDs |
| `myplc_scada_reconnects_total` | Connection drops |
| `myplc_ide_clients` | IDEs connected |
| `myplc_up` | 1 = backend alive |

---

## 6. Test scenarios

| # | Do this | Expect on the dashboard |
|---|---|---|
| 1 | Press **Run** in the IDE | PLC → RUNNING; scans per second ≈ 10 |
| 2 | Press **Stop** | PLC → STOPPED; scans per second → 0 |
| 3 | Stop mock-scada (Ctrl+C) | SCADA link → DOWN; age of last message keeps rising; reconnects +1 |
| 4 | Start mock-scada again | SCADA link → CONNECTED within about 2 s; values per second back |
| 5 | Change the scan time in the project (e.g. 50 ms) and Run | Scan period ≈ 50 ms; scans per second ≈ 20 |
| 6 | Run for 1 hour | Longest scan and jitter stay low; no reconnects |
| 7 | Close the IDE | IDE clients → 0; PLC keeps RUNNING |

Record the results next to the baseline in section 4.3.

---

## 7. Metric reference

All metrics are at `http://<backend address>/metrics` in Prometheus text format.

| Metric | Type | Meaning |
|---|---|---|
| `myplc_up` | gauge | Always 1 while the backend answers |
| `myplc_running` | gauge | 1 while the PLC is RUNNING |
| `myplc_scans_total` | counter | Scans completed |
| `myplc_scan_duration_seconds` | gauge | Time the last scan's work took |
| `myplc_scan_duration_max_seconds` | gauge | Longest scan since start |
| `myplc_scan_period_seconds` | gauge | Time between the starts of the last two scans |
| `myplc_scan_time_setting_seconds` | gauge | Configured scan time |
| `myplc_ide_clients` | gauge | IDEs connected |
| `myplc_scada_connected` | gauge | 1 while connected to the SCADA WebSocket |
| `myplc_scada_reconnects_total` | counter | SCADA connections lost |
| `myplc_scada_messages_total` | counter | SCADA messages received |
| `myplc_scada_values_total` | counter | Tag values applied |
| `myplc_scada_unknown_ids_total` | counter | Tag IDs not found in `st_tags` |
| `myplc_scada_process_seconds` | gauge | Time to read and apply the last SCADA message |
| `myplc_scada_last_message_age_seconds` | gauge | Seconds since the last SCADA message (-1 = none yet) |

Counters only go up (they reset when the backend restarts); use `rate(...[1m])` for "per second". The max and counters restart from 0 when the backend restarts.

---

## 8. Troubleshooting

| Problem | Fix |
|---|---|
| `http://127.0.0.1:5020/metrics` doesn't open | The backend isn't running, or uses another port. Start it (3.3). |
| `--listen needs a value` | Add the address: `--listen 127.0.0.1:5020` |
| Prometheus target **DOWN** | The backend isn't running on `127.0.0.1:5020`; or change `targets` in `monitoring/prometheus.yml` and restart Prometheus. |
| `localhost:3000` doesn't open | Wait a minute after installing; or Windows → Services → **Grafana** → Start. |
| Grafana "Save & test" fails | Prometheus isn't running (3.4), or the URL isn't `http://localhost:9090`. |
| Dashboard says "No data" | Check the Prometheus target is UP; set the time range to "Last 15 minutes". |
| Everything is 0 | PLC not running (press Run), or no SCADA connected (start mock-scada, check `MYPLC_SCADA_URL`). |
| SCADA link stays DOWN | Wrong `MYPLC_SCADA_URL` in `server/.env`, or the SCADA server is offline. Restart the backend after changing `.env`. |
| Unknown tag IDs > 0 | The SCADA sends IDs missing from `st_tags`; the backend log lists them. Check `tagmast` and re-run `server/sql/st_tags.sql`. |

---

## 9. Notes

- `/metrics` needs the token when the backend runs with one (`MYPLC_TOKEN`): scrape `http://<addr>/metrics?token=…` (in `prometheus.yml`, add `params: { token: ["…"] }` under the job). On one PC with no token it's open, as here. The health check stays open either way.
- Prometheus keeps 15 days of history in a `data` folder next to where it was started.
- To stop: Ctrl+C in each terminal. Grafana keeps running as a Windows service (Services → Grafana → Stop to stop it).
