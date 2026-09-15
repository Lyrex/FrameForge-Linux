import React from "react";
import ReactDOM from "react-dom/client";
import { mark } from "./startupMark";
import App from "./App";

mark(`webview script running ${window.location.hash || "main"}`);

let commits = 0;
const onRender: React.ProfilerOnRenderCallback = (_id, phase, actualDuration) => {
  // Startup only; later commits are steady-state noise.
  if (commits++ >= 10) return;
  mark(`commit ${phase} render_ms=${actualDuration.toFixed(0)}`);
};

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <React.Profiler id="root" onRender={onRender}>
      <App />
    </React.Profiler>
  </React.StrictMode>,
);
