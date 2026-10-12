# MyPLC

## Local PC

```
Server run   : cd server && cargo run -- --listen 127.0.0.1:5020
Server build : cd server && cargo build --release
```

## Deployed (Render)

```
Server build : cargo build --release
Server run   : ./target/release/myplc-runtime --listen 0.0.0.0:$PORT
```

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
