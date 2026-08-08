// Self-contained on purpose: state, persistence and UI all live here so the
// upstream sync only ever sees a two-line touch in App.tsx. Persistence rides
// the existing `save_settings` command, which merges the given keys over the
// settings file, so saving `{ eeLogPath }` alone cannot clobber other
// settings and other saves cannot clobber it.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface EeLogStatus {
  detected: string | null;
  exists: boolean;
}

export default function EeLogSettings() {
  // Committed value and live input are separate so status checks and saves
  // run on blur, not per keystroke. `path === null` means the saved value has
  // not loaded yet.
  const [path, setPath] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [status, setStatus] = useState<EeLogStatus | null>(null);

  useEffect(() => {
    invoke<string>("load_settings")
      .then(json => {
        let saved = "";
        try {
          const s = JSON.parse(json);
          if (typeof s.eeLogPath === "string") saved = s.eeLogPath;
        } catch { /* absent or corrupt settings file; start blank */ }
        setPath(saved);
        setDraft(saved);
      })
      .catch(() => setPath(""));
  }, []);

  useEffect(() => {
    if (path === null) return;
    let stale = false;
    invoke<EeLogStatus>("get_ee_log_status", { path })
      // With an override set the backend skips detection and sends detected:
      // null; keep the last detected path so the placeholder survives the
      // player clearing the field.
      .then(s => { if (!stale) setStatus(prev => ({ ...s, detected: s.detected ?? prev?.detected ?? null })); })
      .catch(() => {});
    return () => { stale = true; };
  }, [path]);

  return (
    <div className="settings-section">
      <div className="settings-section-title">Game Log</div>
      <div style={{ fontSize: 11, color: "var(--muted)", marginBottom: 8, lineHeight: 1.5 }}>
        Trade, riven and reward detection all read Warframe's <code style={{ fontSize: 10 }}>EE.log</code>.
        Steam, Flatpak Steam, Lutris and plain Wine prefixes are found automatically; set a path here if yours is somewhere else.
      </div>
      <div className="settings-row">
        <div className="settings-row-info" style={{ flex: 1 }}>
          <span className="settings-row-label">EE.log path</span>
          <input
            type="text"
            value={draft}
            spellCheck={false}
            placeholder={status?.detected ?? "Detecting…"}
            style={{ width: "100%", marginTop: 4, fontSize: 11, fontFamily: "monospace", background: "var(--surface)", border: "1px solid var(--border)", borderRadius: 4, color: "var(--text)", padding: "4px 6px" }}
            onChange={e => setDraft(e.target.value)}
            onBlur={e => {
              const next = e.target.value.trim();
              setDraft(next);
              if (next === path) return;
              setPath(next);
              invoke("save_settings", { json: JSON.stringify({ eeLogPath: next }) }).catch(e2 => {
                console.error("Failed to save EE.log path:", e2);
              });
            }}
          />
          <span className="settings-row-desc" style={{ marginTop: 4 }}>
            Leave empty to auto-detect. Takes effect after a restart.
            {status && !status.exists && (
              (path ?? "").trim() !== ""
                ? <span style={{ color: "#f0c040" }}> No file there yet. Expected if Warframe has not run since you set this; otherwise check the path.</span>
                : <span style={{ color: "#f0c040" }}> Nothing at the detected path. Normal before Warframe's first run; set a path here if the game lives somewhere else.</span>
            )}
          </span>
        </div>
      </div>
    </div>
  );
}
