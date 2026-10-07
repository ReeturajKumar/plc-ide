// Builds the release `myplc-runtime` and puts it where Tauri's bundler takes sidecars from:
// ui/src-tauri/binaries/myplc-runtime-<target triple>[.exe]. The packaged app then installs
// it next to the IDE executable as `myplc-runtime[.exe]` (see tauri.bundle.conf.json).
// Run by `npm run package`; never needed for `tauri dev`.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ui = join(dirname(fileURLToPath(import.meta.url)), "..");
const tauri = join(ui, "src-tauri");
const workspace = join(ui, ".."); // the Cargo workspace: its target/ holds every member's build

execFileSync("cargo", ["build", "--release", "-p", "myplc-runtime", "--manifest-path", join(tauri, "Cargo.toml")], { stdio: "inherit" });

const triple = /^host: (\S+)$/m.exec(execFileSync("rustc", ["-vV"]).toString())?.[1];
if (!triple) throw new Error("rustc -vV did not report a host target triple");
const ext = process.platform === "win32" ? ".exe" : "";
const from = join(workspace, "target", "release", `myplc-runtime${ext}`);
const to = join(tauri, "binaries", `myplc-runtime-${triple}${ext}`);
mkdirSync(dirname(to), { recursive: true });
copyFileSync(from, to);
console.log(`myplc-runtime → ${to}`);
