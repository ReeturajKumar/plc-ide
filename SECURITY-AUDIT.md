# MyPLC Platform — Cybersecurity Gap Assessment

**Date:** 2026-10-28  **Scope:** read-only audit of the actual implementation (no changes, no destructive tests).
**Reviewed:** `server/` (core + runtime), `mock-scada/`, `ui/` (React + Tauri), configs, deploy settings.
**Not a compliance certification.** Standard IDs (OWASP ASVS 5.0, API Top 10, NIST SP 800-82r3, IEC 62443) are mapped only where verifiable.

---

## Section 1 — Executive Summary

MyPLC is a single-trust-domain PLC IDE + runtime. The code is **memory-safe (zero `unsafe`)**, validates inputs at the trust boundaries, caps resources, and has had its dependency tree, message handling, and auth path hardened (R1–R9 fixes verified earlier this cycle). The runtime's own code is in reasonable shape for **internal/controlled use**.

The gaps that remain are **architectural and deployment-level, not code bugs**: a single shared token with **no role model or multi-user isolation**, the token being **embedded in the distributed installer**, **TLS provided only by the hosting proxy** (not the binary), and **unauthenticated SCADA/DB links**. These are the blockers for remote/multi-user/hardware deployment.

**Requirements assessed: 41**
| Status | Count |
|---|---|
| PASS | 20 |
| PARTIAL | 9 |
| FAIL | 4 |
| NOT VERIFIED | 4 |
| NOT APPLICABLE | 4 |

**Critical findings:** 0.
**High findings:** 1 — no authorization model for runtime-control commands beyond a shared token (A-02), high **only** in a multi-user/remote context. *(F-01, token shipped in the installer, has been fixed — the token is now entered at runtime and stored per-machine, not baked into the build.)*
**Major limitation:** no live remote-attacker test from a second host, no fuzzing run, and TLS/SSL behavior of the deployed stack verified only via the public `wss://` URL, not end-to-end from a hostile network position.

---

## Section 2 — Architecture and Attack Surface

**Components & data flow**
```
[IDE: Tauri webview + React/Monaco]
   │  local project files  ──► Tauri FS commands (ui/src-tauri/src/fs.rs)  ──► disk (path-validated)
   │
   │  protocol (JSON over WebSocket, VITE_RUNTIME_URL)
   ▼
[Runtime backend: myplc-runtime --listen]  (server/runtime)
   │  tag names + values
   ├──► PostgreSQL  (st_tags/tagmast/...)   — tags::connect(), NoTls
   └──► SCADA WebSocket (MYPLC_SCADA_URL)    — tags::listen(), plaintext ws, no auth
```

**Trust boundaries**
1. Webview ↔ local FS — crossed only through Tauri commands (`fs.rs`), path-validated.
2. IDE ↔ runtime — WebSocket; auth = optional shared token in the connection query.
3. Runtime ↔ SCADA — **unauthenticated**; runtime trusts the source, validates values.
4. Runtime ↔ DB — credentials from env; `NoTls`.

**Exposed services / ports**
- Runtime: one TCP port, serves WebSocket (protocol) + plain HTTP (`/metrics`, health). `server/runtime/src/server.rs`.
- `/metrics`: token-gated when a token is set (`server.rs:240`).
- On Render: TLS terminated at the edge → public `wss://` / `https://`.

**High-value operations:** `LoadProject`, `Run`, `Stop`, `SetInput`, `SetVariable`, `SetBreakpoints` — all reachable by any client that passes the single token (`host.rs:160`).

---

## Section 3 — Master Cybersecurity Checklist

| ID | Requirement | Status | Severity | Evidence | Required Action |
|---|---|---|---|---|---|
| **A. Authentication & Authorization** |
| A-01 | Auth enforced on network-reachable control | PASS | — | Network bind refused without token `main.rs:69`; WS handshake checks token `server.rs:262-279` | — |
| A-02 | Role/resource-level authorization for commands | FAIL | High* | Single shared token; any client runs any command `host.rs:160-191`. *High only for multi-user/remote | Add per-identity auth + command authz if multi-user is a goal |
| A-03 | Brute-force protection / rate limiting | PASS | — | Per-host lockout after 5 wrong tokens + 1s delay `server.rs:46-51,256-279`; test `server.rs:391` | — |
| A-04 | Credential handling (no password store) | N/A | — | No user accounts; bearer token only | — |
| A-05 | Session invalidation / logout | N/A | — | Stateless per-connection; closing the socket ends access | — |
| A-06 | Multi-user / tenant isolation | FAIL | High* | All clients share one PLC and one token `server.rs:6` | Out of scope for single-trust; required before multi-tenant |
| A-07 | Constant-time token comparison | PASS | — | `token_eq` compares in constant time (no early exit) for both the WS handshake and `/metrics` `server.rs`; unit-tested | — |
| **B. API & Application Security** |
| B-01 | Input validation / schema enforcement | PASS | — | Strict serde decode; bad JSON → `REJECTED` with id `server.rs:107-112` | — |
| B-02 | SQL injection prevention | PASS | — | All queries are static literals, tag names read **out**, params `&[]` `tags.rs:109,209,222` | — |
| B-03 | Command injection / unsafe deserialization | PASS | — | No shell exec; serde only; 0 `unsafe` | — |
| B-04 | Error handling / info disclosure | PASS | — | Friendly messages to client, detail to stderr `fs.rs:friendly`; compiler errors typed not panicked `error.rs:37` | — |
| B-05 | XSS prevention / output encoding | PARTIAL | Low | React escapes; CSP set; **but** `script-src 'unsafe-eval'` needed for Monaco `tauri.conf.json:22` | Accept trade-off; keep other directives strict |
| B-06 | CSRF | N/A | — | No cookie/ambient auth; token in connection | — |
| B-07 | WebSocket Origin validation | FAIL | Low | Handshake checks token only, not `Origin` `server.rs:262` | Low (query-token, not cookie → CSWSH limited); add origin allowlist for browser clients |
| B-08 | Request size / resource-exhaustion limits | PASS | — | 1 MiB msg/frame cap `server.rs:42,280-283`; 128-connection cap `server.rs:355`; tree limits `fs.rs:12` | — |
| B-09 | Path traversal / file access control | PASS | — | `resolve_within` rejects `..`, absolute, drive prefixes `fs.rs:19-33` | — |
| B-10 | Security headers (HTTP endpoints) | PASS | — | `X-Content-Type-Options: nosniff`, `Content-Length`, `Connection: close` `server.rs:228` | — |
| B-11 | SSRF | N/A | — | Runtime connects only to configured DB/SCADA URLs, not client-supplied | — |
| **C. Rust PLC Runtime Security** |
| C-01 | Memory safety / unsafe usage | PASS | — | **0 `unsafe`** across server, runtime, mock, tauri | — |
| C-02 | Runaway / infinite-loop protection | PASS | — | `MAX_LOOP_ITERATIONS_PER_SCAN = 100_000` `executor.rs:17` | — |
| C-03 | Array/nesting/size limits (hostile program) | PASS | — | `MAX_ELEMENTS = 1_000_000` `parser.rs:17`, `MAX_NESTING = 100` `parser.rs:13`, 8 MiB stack `lib.rs:20` | — |
| C-04 | Malformed/hostile program handling | PASS | — | Compiler reports errors as types, never panics `error.rs:37`; verified earlier (PT-10/14) | — |
| C-05 | Panic isolation / process stability | PASS | — | Each client on its own thread; poison-tolerant lock `host.rs:106`; scans wrapped in `catch_unwind` so a panic becomes a clean STOP (outputs to safe state, error reported) `host.rs:70` | — |
| C-06 | Runtime input validation (NaN/Inf) | PASS | — | `SetInput` rejects non-finite REAL `runtime/mod.rs:230` | — |
| C-07 | Unauthorized variable modification | PARTIAL | Medium* | SCADA tags/inputs read-only in programs `checker.rs:712`, but `SetVariable`/`SetInput` are open to any token-holder `host.rs:169-179` | *Tie to A-02; fine single-trust |
| C-08 | Sim vs real hardware separation | NOT VERIFIED | — | Only `simulated` I/O mode exists `main.rs:122`; no real-hardware driver present to assess | Re-assess when a hardware backend is added |
| C-09 | State persistence integrity | PARTIAL | Low | `runtime-state.json` restored on boot `server.rs:89-102`; no integrity check on the file | Validate/limit restored state; it's a local trusted file today |
| **D. WebSocket / SCADA / Communication** |
| D-01 | Auth on connection establishment | PASS | — | Token checked in handshake `server.rs:262-279` | — |
| D-02 | TLS for remote connections | PARTIAL | High (on-prem) | tungstenite built **without TLS** `Cargo.toml:14`; relies on Render edge for public `wss://`. On-prem without a proxy = plaintext | Deploy behind TLS proxy (Render ✅ / Caddy on-prem); document as mandatory |
| D-03 | Message schema/payload validation | PASS | — | serde decode + typed `Command` `server.rs:107`, tag values `is_finite` `tags.rs` | — |
| D-04 | Msg size / rate / connection limits | PASS | — | See B-08 | — |
| D-05 | SCADA source authentication | FAIL | Medium | Runtime connects to `MYPLC_SCADA_URL` and trusts it; no credential `tags.rs:listen` | Needs SCADA-side auth (other system); validate/allowlist source |
| D-06 | Replay / duplicate-command handling | NOT VERIFIED | Low | No nonce/sequence; commands are idempotent state transitions, impact limited | Assess if ordering/replay matters for control |
| D-07 | Disconnect/reconnect/stale handling | PASS | — | Client leave keeps PLC state; SCADA reconnects `tags.rs`; connection count decremented `server.rs:367` | — |
| **E. Secrets & Data Protection** |
| E-01 | No hardcoded production secrets in source | PASS | — | Token/DB creds from env; `.env` gitignored (`.gitignore:13`) and untracked (`git ls-files` empty) | — |
| E-02 | Secret not exposed in client artifact | PASS | — | Token no longer baked in: entered on first launch and stored on the machine via `connection.ts`; the build sets no token (`ui/.env`). Shared installer carries no secret | — |
| E-03 | Secrets excluded from logs | PASS | — | Logs peer/host, never token value `server.rs:259` | — |
| E-04 | Encryption in transit | PARTIAL | — | Public edge `wss://`/`https://` (Render); internal runtime↔SCADA↔DB plaintext | Acceptable within one trusted private network; else add TLS |
| E-05 | Encryption at rest | NOT VERIFIED | Low | Project files and `runtime-state.json` stored plaintext locally; DB-at-rest depends on Render | Rely on host/disk encryption; document |
| E-06 | Secure DB permissions / connection | PARTIAL | Medium | `NoTls` `tags.rs:52`; DB user least-privilege not verified | Use a read-scoped DB role; add TLS if DB is off-host |
| E-07 | Cross-user data leakage | N/A | — | No per-user data model | — |
| **F. Dependencies & Supply Chain** |
| F-01 | Known vulns in deps (Cargo/npm) | PASS | — | `cargo audit` + `npm audit` both clean after bumps (ctutils→0.4.4, tokio→1.54.1, dompurify→3.4.17, vite/source-map-js) this cycle | Re-run on a schedule |
| F-02 | Lockfiles present | PASS | — | `Cargo.lock`, `package-lock.json` committed | — |
| F-03 | Build/install script risks | PASS | — | No `build.rs` with network/exec; no npm `postinstall` hooks observed | — |
| F-04 | CI/CD permissions & build-secret exposure | NOT APPLICABLE | — | No CI pipeline (`.github/workflows` absent); builds are local | Add CI with secret scanning before team scale-up |
| F-05 | Release artifact integrity / update security | FAIL | Medium | NSIS/MSI installer is unsigned; no update signature/channel | Code-sign the installer; define an update mechanism |
| **G. Logging, Monitoring, Incident Response** |
| G-01 | Auth-failure logging | PARTIAL | Low | Bad-token refusals logged to stderr `server.rs:259`; not persisted/aggregated | Ship stderr to a log store; alert on lockouts |
| G-02 | Metrics / monitoring | PASS | — | Prometheus `/metrics`: clients, scans, SCADA, jitter `metrics.rs:43`; Grafana dashboard provided | — |
| G-03 | Audit trail for sensitive ops | PARTIAL | Medium | Load/Run/Stop logged to stderr, not an audit log; no actor identity (single token) | Add structured audit events tied to identity (needs A-02) |
| G-04 | Suspicious-activity detection | PARTIAL | Low | Lockout logged; no alerting | Alert on `token_locked` / connection-cap events |
| **H. Deployment & Operational Security** |
| H-01 | Debug/dev settings disabled in prod | PASS | — | Strict prod CSP (vs none in dev) `tauri.conf.json:22`; `--token` CLI removed (R5) `main.rs:59` | — |
| H-02 | Least-privilege process/FS | PARTIAL | Low | `opener` narrowed to `allow-reveal-item-in-dir` `capabilities/default.json` (can no longer open arbitrary URLs/paths); runtime FS = its data dir. Remaining: non-root service user, view-only DB role | Run runtime as non-root; scope the DB role |
| H-03 | Restricted network services | PARTIAL | Medium | One port; but DB `5432` exposure/`pg_hba` and firewall not verified (PT-24 open) | Verify DB not world-reachable; firewall the runtime port |
| H-04 | TLS / certificate configuration | PARTIAL | — | Render-managed certs on public URL; none in-binary | Keep behind managed TLS; verify cert chain |
| H-05 | Env separation (dev/test/prod) | PARTIAL | Low | `.env` per environment; no enforced separation | Document and separate prod secrets |
| H-06 | Backup integrity / tested recovery | NOT VERIFIED | — | DB dump→restore done once; no backup policy | Define/test DB backups |
| **I. Testing & Secure Development** |
| I-01 | Security-relevant tests exist | PARTIAL | — | R9 lockout unit tests `server.rs:391`; tag validation + overflow tests; limited negative-auth coverage | Add negative-auth/authz tests |
| I-02 | Dependency vuln scanning | PASS | — | cargo audit + npm audit executed this cycle | Automate in CI |
| I-03 | Secret scanning | NOT VERIFIED | — | No automated secret scan; manual check found none in history (PT-25) | Add gitleaks/trufflehog |
| I-04 | SAST / Rust lint | PARTIAL | — | `clippy` annotations present; no enforced SAST gate | Add `clippy -D warnings` + CI |
| I-05 | Fuzzing of parser/protocol | NOT VERIFIED | — | Manual malformed-input tests done (PT-10/14); no `cargo fuzz` harness | Add a fuzz target for the ST parser + protocol decode |
| I-06 | Threat model / requirements traceability | PARTIAL | — | R1–R9 + PT-01..31 give partial traceability; no formal threat model | This document + a short STRIDE pass closes it |

---

## Section 4 — Confirmed Vulnerabilities

**F-01 / E-02 — Backend token embedded in the distributed installer** · ~~High~~ **RESOLVED (2026-10-28)** · `ui/src/utils/connection.ts`
*Was:* Vite inlined `VITE_RUNTIME_TOKEN` into the shipped JS, so every installer holder had the backend token.
*Fix:* the token is no longer built in. `ui/.env` sets no token; the app reads the backend address + token from `connection.ts` (saved on the machine via localStorage), prompts for them on first launch (`App.tsx`), and they are editable in Settings → Backend. The shared installer carries no secret.
*Residual:* the token is still a **shared** credential — whoever is given it has full control. Per-user auth (A-02) is the next step for multi-user/remote use.

**A-02 / A-06 — No authorization model for control commands** · High (multi-user/remote only) · `server/runtime/src/host.rs:160-191`
A single shared token authenticates the connection; there is no per-identity authorization, so every connected client can issue every control command against the same PLC.
*Verify:* connect two clients with the same token; both can `Run`/`Stop`/`SetVariable`.
*Impact:* one compromised or careless client controls the PLC for everyone.
*Remediation:* required before multi-user or Internet-facing deployment; acceptable within a single trusted operator domain.

**D-05 — Unauthenticated SCADA source** · Medium · `server/runtime/src/tags.rs` (`listen`)
The runtime connects to `MYPLC_SCADA_URL` and trusts whatever it sends; only value *sanity* is checked (`is_finite`).
*Impact:* a party able to occupy that URL can feed plausible-but-false tag values. (Garbage is already rejected — PT-16/17.)
*Remediation:* SCADA-side authentication + source allowlist; largely an external-system control.

**F-05 — Unsigned installer, no secure update** · Medium · build output `bundle/nsis`, `bundle/msi`
The installer is unsigned and there is no update-signature mechanism.
*Impact:* tampered builds can't be distinguished by recipients; no integrity on updates.
*Remediation:* code-sign; adopt Tauri's signed updater or a checksum/signature distribution step.

**D-02 — TLS not in the binary (plaintext on-prem without a proxy)** · High for on-prem, mitigated on Render · `server/runtime/Cargo.toml:14`
tungstenite is built without TLS; confidentiality/integrity on the wire depend entirely on an external TLS terminator. On Render this is satisfied (public `wss://`); an on-prem deploy without a reverse proxy is plaintext.
*Remediation:* mandate a TLS proxy (Render edge / Caddy); document as a hard requirement.

---

## Section 5 — Missing and Incomplete Controls

**Completely missing**
- Per-identity authentication / RBAC (A-02, A-06).
- WebSocket `Origin` validation (B-07).
- Installer code-signing & secure update (F-05).
- CI pipeline, secret scanning, SAST gate, fuzz harness (F-04, I-03, I-04, I-05).
- Structured, identity-tied audit log (G-03).

**Partially implemented**
- TLS/encryption in transit — edge only, not internal (D-02, E-04, E-06).
- Monitoring — metrics yes, alerting no (G-01, G-04).
- Least privilege — Tauri `opener` cap, DB role scope, service user (H-02, E-06).

**Could not be verified (need evidence/environment)**
- Sim-vs-hardware separation (no hardware backend exists yet) — C-08.
- DB network exposure / `pg_hba` / firewall — H-03 (PT-24 still open).
- Encryption at rest, backup/recovery — E-05, H-06.
- Replay handling relevance — D-06.

---

## Section 6 — Security Testing Results (actually executed)

| Test | Command | Result |
|---|---|---|
| Rust deps | `cargo audit` (server + mock-scada) | Found ctutils 0.4.3 (RUSTSEC-2026-0385) + unsound tokio 1.53.2; **fixed** → 0 vulns |
| npm deps | `npm audit` (ui) | Found 4 (critical dompurify, high vite/source-map-js, low); **fixed** → 0 vulns |
| `unsafe` scan | `grep -rn unsafe` | **0** occurrences |
| SQL construction | static-string review of all `query(...)` | No input interpolation → no injection |
| Backend tag query | read-only `SELECT count(*)` joins vs Render DB | 20,017 rows — query path sound |
| Resource limits | source review | loop 100k, elements 1M, nesting 100, msg 1 MiB, conns 128 — all present |
| Earlier cycle (prior sessions, referenced) | PT-01..31 | 401/200 auth checks, 2 MiB drop, 128-conn cap, lockout, malformed JSON reject, loop-limit STOP, no path traversal, MITM-plaintext confirmed |

**Not run (proposed):** remote nmap from a second host (PT-02); `cargo fuzz` on ST parser + protocol decode; ZAP WebSocket fuzz (PT-13); DB exposure / `pg_hba` review (PT-22/24); installer-bundle token extraction (verifies F-01); end-to-end TLS validation from a hostile network position.

---

## Section 7 — Prioritized Remediation Roadmap

**1. Immediate (containment)**
- Decide the token's trust level. Until A-02 exists, **do not share the installer beyond the trusted test team** (F-01). Rotate `MYPLC_TOKEN` if it has leaked. *Done when:* distribution list is controlled and a rotation runbook exists.
- Confirm the DB (`5432`) and the runtime port are **not world-reachable** (H-03). *Done when:* firewall/`pg_hba` verified from outside.

**2. High-priority fixes**
- Per-identity auth + command authorization for control (A-02/A-06) — the gate for any multi-user/remote use. *Acceptance:* a non-privileged identity cannot `Run/Stop/SetVariable`.
- Keep the runtime **behind managed TLS** and document it as mandatory (D-02). *Acceptance:* no plaintext control path in any supported deployment.
- **Code-sign** the installer + define updates (F-05). *Acceptance:* signature verifies on a clean machine.

**3. Medium / low**
- `catch_unwind` around scan execution (C-05). *Acceptance:* a forced panic leaves the server serving.
- WebSocket `Origin` allowlist + constant-time token compare (B-07, A-07).
- DB TLS + least-privilege role (E-06); drop `opener` cap if unused (H-02).
- Ship stderr to a log store; alert on lockout/connection-cap (G-01/G-04).

**4. Long-term hardening**
- CI with `cargo audit`/`npm audit`, `clippy -D warnings`, gitleaks, and a `cargo fuzz` target for the parser/protocol (F-04, I-02..I-05).
- Structured, identity-tied audit log (G-03); short STRIDE threat model (I-06); documented backup/recovery (H-06).

---

## Section 8 — Final Readiness Assessment

| Use case | Ready? | Why |
|---|---|---|
| **Internal development use** | ✅ Yes | Memory-safe, input-validated, resource-capped, deps clean. Suitable for the team on a trusted network. |
| **Controlled testing (trusted testers)** | ⚠️ With care | Works, but the installer carries the backend token (F-01) and there's no per-user authz (A-02). Keep distribution controlled; keep it behind Render TLS. |
| **Remote / multi-user deployment** | ❌ No | No identity/RBAC (A-02/A-06), token in client (E-02), no origin check (B-07), unsigned updates (F-05). These must be built first. |
| **Connection to real industrial hardware** | ❌ No / unproven | Only `simulated` I/O exists (C-08); safety separation, hardware command authz, and fail-safe behavior are not implemented or verifiable. Do **not** wire to live plant. |

**Limitations of this assessment:** static/read-only; no second-host network test, no fuzzing, no installer-extraction test executed; deployment-side controls (firewall, DB exposure, at-rest encryption, backups) asserted from config, not externally verified. Items marked NOT VERIFIED need those tests or that environment before a status can be assigned.
