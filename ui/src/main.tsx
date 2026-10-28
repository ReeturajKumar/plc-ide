import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

// Bundle Monaco locally instead of @monaco-editor/react's default CDN load, so the
// editor works offline and under the packaged app's strict CSP (no cdn.jsdelivr.net).
import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";

(self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = {
  getWorker: () => new EditorWorker(),
};
loader.config({ monaco });

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
