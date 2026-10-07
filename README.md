# MyPLC

An IEC 61131-3 PLC IDE (Structured Text) with a separate PLC runtime.

```
ui/         What the user interacts with: the IDE (React + Monaco in ui/src,
            Tauri desktop shell in ui/src-tauri, crate `my-plc-ide`)
runtime/    The standalone PLC runtime process (`myplc-runtime`)
core/       The PLC engine (`myplc-core`): compiler, runtime engine, I/O, protocol
```

```
UI ──protocol (JSON over stdio)──► runtime ──► core
 └──────────────── compiler queries ─────────► core
```

`core` depends on neither `ui` nor `runtime`; `runtime` depends only on `core`. The UI talks
to the runtime only through the protocol (`core/src/protocol`, mirrored in
`ui/src/types/protocol.ts`).

## Develop

```bash
cd ui
npm install
npm run tauri dev
```

`npm run dev` builds `myplc-runtime` first; the IDE finds it next to its own executable
in `target/`.

Run the runtime on its own:

```bash
cargo run -p myplc-runtime -- path/to/project --trace
```

## Package (Windows)

```bash
cd ui
npm run package
```

Builds the release runtime and the IDE, and produces installers in
`target/release/bundle/` with `myplc-runtime.exe` beside `my-plc-ide.exe`.
