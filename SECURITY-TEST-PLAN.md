# MyPLC Penetration Test Plan

A plan to attack MyPLC on purpose, with permission, to find security holes before a real attacker does.

> **Rules:** test only machines and networks you own or have written permission to test. Never test the colleague's SCADA server (`192.168.1.251`) or the shared database without their written OK. Run tests against a **test instance** (e.g. port 5021 with its own `--data` folder), not the PLC in use.

---

## 1. Scope

| In scope | Address (test setup) |
|---|---|
| MyPLC backend: WebSocket API, health check, `/metrics` | `ws://<test PC>:5021`, `http://<test PC>:5021/metrics` |
| SCADA link (backend as a WebSocket client) | mock-scada `ws://<test PC>:6001` |
| PostgreSQL tag database | `<test PC>:5432` |
| IDE desktop app (Tauri) | `ui/` |
| Dependencies (Rust crates, npm packages) | `server/`, `mock-scada/`, `ui/` |
| Configuration and secrets | `server/.env`, data folder, logs |

**Out of scope:** the colleague's SCADA server, production databases, denial-of-service on shared networks, social engineering.

---

## 2. Tools (all free)

| Tool | Used for |
|---|---|
| **Nmap** | Finding open ports and services |
| **OWASP ZAP** | WebSocket and HTTP attacks, fuzzing |
| **Burp Suite Community** | Capturing and changing WebSocket messages by hand |
| **websocat** | Sending WebSocket messages from the command line |
| **cargo-audit** | Known vulnerabilities in Rust crates |
| **npm audit** | Known vulnerabilities in npm packages |
| **Wireshark** | Checking whether traffic is readable on the network |
| **psql / pgAdmin** | Database permission checks |

---

## 3. Known risks (found by code review, to confirm in testing)

| # | Risk | Severity | Status | Where |
|---|---|---|---|---|
| R1 | No token by default: anyone who reaches the port can load programs, Run or Stop the PLC | **Critical** | ✅ Fixed | `server/runtime/src/main.rs` |
| R2 | No encryption: `ws://` and `http://` only; the token and all data travel in clear text | **High** | Mitigated — run behind TLS (reverse proxy / platform); documented in README. Runtime stays plain `ws` on localhost | backend, SCADA link |
| R3 | SCADA link has no authentication: anyone who can serve the SCADA URL (or sit in the middle) can feed fake tag values into the PLC | **High** | Partly — garbage/non-finite values now rejected; MITM closed by `wss://` (R2). SCADA proving its identity needs that server to support auth | `server/runtime/src/tags.rs` |
| R4 | `/metrics` needs no token | Medium | ✅ Fixed — needs the token when one is set; health check stays open | `server/runtime/src/server.rs` |
| R5 | Token given on the command line (`--token`): visible in the process list and shell history | Medium | ✅ Fixed — `--token` removed; token only from `MYPLC_TOKEN` | `server/runtime/src/main.rs` |
| R6 | DB password stored in plain text in `server/.env` | Medium | Partly — `.env` is gitignored (never committed); OS file-permission / credential-store hardening still to do | `server/.env` |
| R7 | Tauri app has no Content Security Policy (`"csp": null`) | Medium | ✅ Fixed — strict CSP set (verify in `tauri dev`) | `ui/src-tauri/tauri.conf.json` |
| R8 | No limit on message size or connection count: large or many messages could slow or crash the backend | Medium | ✅ Fixed — 1 MiB message cap, 128-connection cap | `server/runtime/src/server.rs` |
| R9 | No rate limit on token guesses | Low | ✅ Fixed — 1 s pause per wrong token, host locked out after 5 in 60 s | `server/runtime/src/server.rs` |

Note: the token still travels in the URL (`?token=…`) because the IDE uses a browser WebSocket, which can't set custom headers. That exposure is closed by TLS (R2), not by R5.

Good news: SQL queries in the backend are fixed (no user input is put into SQL), so SQL injection risk is low; it's still tested below.

---

## 4. Test cases

Result column: **Pass** = attack failed, **Fail** = attack worked (a vulnerability), **N/A** = not tested.

### 4.1 Discovery

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-01 | Open ports | `nmap -sV -p- <test PC>` | Only the expected ports are open (backend, PostgreSQL if remote access is needed) | |
| PT-02 | Ports reachable from another PC | Run PT-01 from a second PC on the network | The backend and DB are reachable only where needed (firewall rules) | |
| PT-03 | Information leaks | Open `http://<test PC>:5021/` and `/metrics`; look at response headers | No versions, paths or secrets exposed beyond what is accepted | |

### 4.2 Authentication and access control

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-04 | Control without a token (default setup) | `websocat ws://<test PC>:5021`, send a `STOP` request | Refused (**expected Fail today: R1**) | |
| PT-05 | Control with a wrong token | Start backend with `MYPLC_TOKEN`; connect with `?token=wrong` | `401`, no command accepted | |
| PT-06 | Control with an empty or missing token | Connect with `?token=` and with no query | `401` | |
| PT-07 | Token guessing | Send 1,000 wrong tokens quickly (script or ZAP fuzzer) | Blocked or slowed after a few tries (**expected Fail today: R9**) | |
| PT-08 | Token exposure | Check the backend log, the process list (`tasklist /v`, Process Explorer), and proxy logs | Token never visible (**expected Fail if `--token` is used: R5**) | |
| PT-09 | `/metrics` without login | `curl http://<test PC>:5021/metrics` from another PC | Accepted risk, or blocked (**R4**) | |

### 4.3 Input handling (WebSocket API)

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-10 | Malformed JSON | Send broken JSON, wrong types, missing fields | Clear error reply; backend keeps running | |
| PT-11 | Huge message | Send a 10 MB and a 100 MB message | Rejected; memory stays normal; backend keeps running (**R8**) | |
| PT-12 | Many connections | Open 500+ connections at once | Backend stays responsive; PLC scans keep their timing (watch Grafana) (**R8**) | |
| PT-13 | Fuzzing | OWASP ZAP WebSocket fuzzer on every command | No crash, no hang, no scan-time spike | |
| PT-14 | Malicious ST program | Load programs with endless loops, very deep nesting, huge arrays | Compile error or scan error; backend stays alive; PLC stops safely | |
| PT-15 | Path tricks | Program/file names like `../../x`, very long names, special characters | Rejected; no file written outside the data folder | |

### 4.4 SCADA link

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-16 | Fake SCADA server | Point `MYPLC_SCADA_URL` at a server you control; send false values | Detected or rejected (**expected Fail today: R3**) | |
| PT-17 | Bad SCADA messages | Send broken JSON, unknown IDs, text instead of numbers, NaN/huge numbers | Ignored and logged; no crash; no wrong values applied | |
| PT-18 | SCADA flood | Send 100 messages per second with 20,000 tags | PLC scan timing stays within limits (watch Grafana) | |
| PT-19 | Sniffing | Wireshark on the network while SCADA sends values | Values unreadable (**expected Fail today: R2**) | |

### 4.5 Network and encryption

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-20 | IDE traffic readable | Wireshark while the IDE connects with a token | Token and programs unreadable (**expected Fail today: R2**) | |
| PT-21 | Man in the middle | Proxy the IDE through Burp and change a request | Detected, or not possible (requires TLS) | |

### 4.6 Database

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-22 | Least privilege | Check the role in `MYPLC_DB_USER` (`\du` in psql) | Read-only on the tables it needs; not a superuser | |
| PT-23 | SQL injection | Tag names with `'; DROP TABLE x; --` in `tagmast` (test DB only), then load tags | No SQL runs; names shown as text | |
| PT-24 | DB reachable from outside | Connect to port 5432 from another PC | Refused unless needed (`pg_hba.conf`, firewall) | |

### 4.7 Secrets and configuration

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-25 | Secrets in git | `git log -p` and search for passwords, tokens | None ever committed | |
| PT-26 | File permissions | Check who can read `server/.env` and the data folder | Only the service account and admins (**R6**) | |
| PT-27 | Secrets in logs | Search logs for password and token values | None found | |

### 4.8 Desktop app (Tauri)

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-28 | Script injection | Put `<img src=x onerror=alert(1)>` in program names, variable names, tag names, comments | Shown as text; nothing runs (**R7** makes this worse if it fails) | |
| PT-29 | App permissions | Review `ui/src-tauri/capabilities/default.json` | Only the permissions the app needs | |

### 4.9 Dependencies

| ID | Test | How | Pass if | Result |
|---|---|---|---|---|
| PT-30 | Rust crates | `cargo install cargo-audit`, then `cargo audit` in `server/` and `mock-scada/` | No known vulnerabilities, or each one assessed | |
| PT-31 | npm packages | `npm audit` in `ui/` | No high/critical issues, or each one assessed | |

---

## 5. Order of work

1. **Prepare:** test instance on port 5021, test DB copy, mock-scada on 6001, Grafana open.
2. **Quick wins (1–2 hours):** PT-30, PT-31, PT-01, PT-04, PT-25.
3. **Main tests:** sections 4.2 to 4.6.
4. **Desktop app:** section 4.8.
5. **Report:** fill in the Result column; list each Fail with its severity and fix.
6. **Fix and retest:** every Fail is fixed, then its test is run again.

---

## 6. Expected fixes (based on the known risks)

| Risk | Fix |
|---|---|
| R1 | Require a token always (refuse to start without one, or only listen on `127.0.0.1` without one) |
| R2 | TLS: `wss://` and `https://` (or a reverse proxy with TLS in front) |
| R3 | Token or TLS certificate check on the SCADA link; sanity limits on values |
| R4 | Token on `/metrics`, or listen for metrics on `127.0.0.1` only |
| R5 | Send the token in a header or first message, not the URL; take it only from `MYPLC_TOKEN` |
| R6 | Restrict file permissions; later, Windows Credential Manager |
| R7 | Set a strict Content Security Policy in `tauri.conf.json` |
| R8 | Maximum message size and maximum connections |
| R9 | Delay or block after repeated wrong tokens |

---

## 7. Report template (per finding)

```
ID:            PT-xx
Title:
Severity:      Critical / High / Medium / Low
Steps:         1. … 2. … 3. …
Result:        what happened
Impact:        what an attacker could do
Fix:
Retest:        date, Pass/Fail
```
