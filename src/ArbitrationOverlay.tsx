import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useOverlayWindow } from "./useOverlayWindow";
import { fmtMs } from "./TimerHelper";
import "./ArbitrationOverlay.css";

interface RunSummary {
  node:             string;
  mission_type:     string;
  duration_sec:     number;
  rotations:        number;
  waves:            number;
  kills:            number;
  drone_kills:      number;
  host_telemetry:   boolean;
  vitus_mean:       number;
  vitus_per_minute: number;
}

// Long enough to read five numbers, short enough that the next mission's
// loading screen is not still wearing them.
const AUTO_HIDE_MS = 12_000;

// The payload is this app's own struct, but a field the render divides or
// formats must exist: a throw here would unmount the overlay and leave the
// window shown with nothing in it.
function isRunSummary(p: unknown): p is RunSummary {
  const r = p as Partial<RunSummary> | null;
  return !!r
    && typeof r.node === "string"
    && typeof r.mission_type === "string"
    && typeof r.duration_sec === "number"
    && typeof r.vitus_mean === "number"
    && typeof r.vitus_per_minute === "number";
}

export default function ArbitrationOverlay() {
  const [run, setRun] = useState<RunSummary | null>(null);
  const root  = useOverlayWindow(340, "top-center");
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const hide = useCallback(() => {
    if (timer.current) { clearTimeout(timer.current); timer.current = null; }
    setRun(null);
    getCurrentWindow().hide().catch(() => {});
  }, []);

  // The backend shows the window before it emits. An event that lands before
  // this listener exists, or one that fails validation, would otherwise leave
  // an empty window over the game with nothing to ever hide it.
  useEffect(() => {
    if (!run) getCurrentWindow().hide().catch(() => {});
  }, [run]);

  useEffect(() => {
    const unEnded = listen<unknown>("arbitration-run-ended", e => {
      if (!isRunSummary(e.payload)) return;
      // A second run ending while the first is still up restarts the clock
      // rather than cutting the new one short.
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(hide, AUTO_HIDE_MS);
      setRun(e.payload);
    });
    return () => {
      unEnded.then(f => f());
      if (timer.current) clearTimeout(timer.current);
    };
  }, [hide]);

  if (!run) return null;

  const [progressValue, progressLabel] = run.mission_type === "defense"
    ? [run.waves, "waves"]
    : [run.rotations, run.rotations === 1 ? "rotation" : "rotations"];

  return (
    <div className="aro-root" ref={root}>
      <div className="aro-header">
        <span className="aro-title">Arbitration Complete</span>
        <span className="aro-node">{run.node}</span>
        <button className="aro-close" onClick={hide} title="Close">✕</button>
      </div>
      <div className="aro-stats">
        <div className="aro-stat">
          <span className="aro-value">{fmtMs(run.duration_sec * 1000)}</span>
          <span className="aro-label">duration</span>
        </div>
        <div className="aro-stat">
          <span className="aro-value">{progressValue}</span>
          <span className="aro-label">{progressLabel}</span>
        </div>
        <div className="aro-stat">
          <span className="aro-value">{run.host_telemetry ? run.kills : "–"}</span>
          <span className="aro-label">kills</span>
        </div>
        <div className="aro-stat">
          <span className="aro-value">{run.host_telemetry ? run.drone_kills : "–"}</span>
          <span className="aro-label">drones</span>
        </div>
        <div className="aro-stat aro-vitus">
          <span className="aro-value">{run.vitus_per_minute.toFixed(2)}</span>
          <span className="aro-label">vitus/min · ~{Math.round(run.vitus_mean)} total</span>
        </div>
      </div>
      {!run.host_telemetry && (
        <div className="aro-note">Kill counts are only logged when hosting.</div>
      )}
    </div>
  );
}
