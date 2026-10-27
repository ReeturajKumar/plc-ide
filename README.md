# MyPLC

## Local PC

```
Server run   : cd server && cargo run -- --listen 127.0.0.1:5020
Server build : cd server && cargo build --release
```

## Mock SCADA (live tag values for testing)

```
Run          : cd mock-scada && cargo run
```

The backend connects to `MYPLC_SCADA_URL` in `server/.env` (`ws://127.0.0.1:6000` = the mock).
Put the real system's address there later.

## Monitoring (Prometheus + Grafana)

The server shows its live numbers (scan time, jitter, SCADA messages and latency) at `http://127.0.0.1:5020/metrics`. Full guide: [MONITORING.md](MONITORING.md).

```
Prometheus : prometheus.exe --config.file=monitoring/prometheus.yml   (http://localhost:9090)
Grafana    : add data source Prometheus http://localhost:9090, then Dashboards > Import > monitoring/grafana-dashboard.json   (http://localhost:3000)
```

## Deployed (Render)

```
Server build : cargo build --release
Server run   : ./target/release/myplc-runtime --listen 0.0.0.0:$PORT
```

A network address (anything but `127.0.0.1`) requires a token: set the `MYPLC_TOKEN`
environment variable (clients then connect with `?token=…`, and `/metrics` needs it too).
The token is taken only from the environment, never a `--token` flag, so it can't leak via
the process list.

## Client (IDE)

```
Client run   : cd ui && npm run tauri dev
Client build : cd ui && npm run package
```

Set the server URL in `ui/.env` before building the client.

## Exe files (to share)

```
Client installer : ui/src-tauri/target/release/bundle/nsis/my-plc-ide_0.1.0_x64-setup.exe
Server           : server/target/release/myplc-runtime.exe
```

## Encryption / TLS (R2)

The runtime speaks plain `ws://`/`http://`. To encrypt the IDE↔backend link (so the token
and data aren't readable on the wire), terminate TLS in front of it rather than in the
runtime:

- **Render / cloud:** TLS is automatic — the public URL is `https://…`, so point the IDE at
  `wss://…` (set `VITE_RUNTIME_URL` in `ui/.env`). Nothing else to do.
- **On-prem:** run a reverse proxy on the same machine. Example `Caddyfile`:

  ```
  plc.example.com {
      reverse_proxy 127.0.0.1:5020
  }
  ```

  Keep the runtime on `127.0.0.1:5020` (local only) and let Caddy hold the cert and serve
  `wss://plc.example.com`. The IDE then uses that `wss://` URL.

**SCADA link (R3):** if the SCADA server offers TLS, set `MYPLC_SCADA_URL=wss://…` to stop
anyone reading or altering values in transit. The runtime already rejects garbage values
(non-finite / wrong-typed), but a SCADA source proving *who it is* depends on that server
supporting authentication.
