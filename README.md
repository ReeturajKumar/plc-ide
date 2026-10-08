# MyPLC

An IEC 61131-3 PLC IDE (Structured Text) with a separate PLC backend.

```
server/            BACKEND   everything that runs on the server (its own Cargo project)
  ├── runtime/               the PLC server program (myplc-runtime): runs 24×7, IDEs connect to it
  └── core/                  the PLC engine: compiler, runtime engine, I/O, protocol
ui/                FRONTEND  the IDE (React + Monaco in ui/src, Tauri desktop shell in ui/src-tauri)
```

`ui/src-tauri` uses `server/core`'s compiler for the editor's live error checking; nothing
else crosses between the two folders except the network connection.

```
 Frontend (ui)  ──── ws://server:5020  (URL in ui/.env) ────►  Backend (runtime)  ──► core
   commands: RUN, STOP, SET_INPUT …            ◄──── live state pushed (STATE_UPDATE)
```

- The frontend reads the backend's address from `ui/.env` (`VITE_RUNTIME_URL`), like any
  React app with an API URL. It never starts the backend.
- The backend keeps running when IDEs disconnect, remembers the loaded project and the
  RUN/STOP mode, and after a restart loads it again (and runs it, if it was running).
- One WebSocket endpoint carries the protocol (`server/core/src/protocol`, mirrored in
  `ui/src/types/protocol.ts`). WebSockets are not subject to CORS; access is controlled by
  the backend's token instead.

## Develop

Terminal 1, the backend:

```bash
cd server
cargo run -- --listen 127.0.0.1:5020
```

Terminal 2, the frontend (first time: `npm install`, and copy `ui/.env.example` to `ui/.env`):

```bash
cd ui
npm run tauri dev
```

## Backend on a server (24×7)

Build it (Windows or Linux; it is a single executable with nothing else to install):

```bash
cd server
cargo build --release
```

Copy `server/target/release/myplc-runtime(.exe)` to the server and run it with a token:

```bash
myplc-runtime --listen 0.0.0.0:5020 --token <secret> --data <folder>
```

- `--listen 0.0.0.0:5020` accepts connections on every network interface; open port 5020
  in the server's firewall for the IDE machines only.
- `--token` (or the `MYPLC_TOKEN` environment variable): IDEs must present it. Without a
  token anyone who reaches the port controls the PLC.
- `--data`: where the loaded project and RUN/STOP mode are kept (default: `myplc-data`
  next to the executable).

Start it with the machine:

- **Windows** (as administrator), a task that starts it at boot (for automatic restarts after
  a crash, set "If the task fails, restart every…" in the task's Settings tab):
  ```bash
  schtasks /create /tn MyPLC /sc onstart /ru SYSTEM /rl HIGHEST /tr "C:\MyPLC\myplc-runtime.exe --listen 0.0.0.0:5020 --token <secret> --data C:\MyPLC\data"
  ```
- **Linux**, `/etc/systemd/system/myplc.service`:
  ```ini
  [Unit]
  Description=MyPLC runtime
  After=network.target

  [Service]
  ExecStart=/opt/myplc/myplc-runtime --listen 0.0.0.0:5020 --data /var/lib/myplc
  Environment=MYPLC_TOKEN=<secret>
  Restart=always

  [Install]
  WantedBy=multi-user.target
  ```
  then `systemctl enable --now myplc`.

## Frontend for users

In `ui/.env`, point the IDE at the server, then build the installer:

```
VITE_RUNTIME_URL=ws://<server-ip>:5020
VITE_RUNTIME_TOKEN=<secret>
```

```bash
cd ui
npm run package
```

The installers are in `ui/src-tauri/target/release/bundle/`. The URL is built into the IDE: rebuild it
when the server address changes.

## Run a project from the command line (no server, no IDE)

```bash
cd server
cargo run -- path/to/project --trace
```
