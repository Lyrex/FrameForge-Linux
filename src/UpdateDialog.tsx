import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import type { UpdateAvailable } from "./updater";
import { errorText } from "./updateFlow";

interface Props {
  update: UpdateAvailable;
  onDismiss: () => void;
}

type Phase = "prompt" | "installing" | "installed" | "failed";

export default function UpdateDialog({ update, onDismiss }: Props) {
  const [phase, setPhase] = useState<Phase>("prompt");
  const [error, setError] = useState("");

  async function install() {
    setPhase("installing");
    try {
      // The launch check reports the new version but keeps no handle on it,
      // so the manifest is fetched again to get one that can be downloaded.
      const handle = await check();
      if (!handle) throw new Error("the release is no longer published");
      await handle.downloadAndInstall();
      // Only Linux gets this far. On Windows the plugin hands the downloaded
      // package to the installer and exits, and the installer starts the new
      // version itself, so there is no restart to offer.
      setPhase("installed");
    } catch (e) {
      setError(errorText(e));
      setPhase("failed");
    }
  }

  const dismissable = phase !== "installing";

  return (
    <div className="ff-modal-overlay" onClick={() => dismissable && onDismiss()}>
      <div className="ff-modal" style={{ width: 480 }} onClick={e => e.stopPropagation()}>
        <div className="riven-modal-title">
          FrameForge {update.version} is available
        </div>

        <div style={{ fontSize: 12, color: "var(--muted)" }}>
          You are running {update.currentVersion}.
        </div>

        {update.notes && (
          // GitHub release notes are Markdown; shown as written rather than
          // rendered, which no dependency here can do.
          <pre style={{
            fontSize: 11, lineHeight: 1.6, margin: 0, maxHeight: 260, overflow: "auto",
            whiteSpace: "pre-wrap", wordBreak: "break-word", color: "var(--muted)",
          }}>{update.notes}</pre>
        )}

        {phase === "installed" && (
          <div style={{ fontSize: 12, color: "var(--green)" }}>
            Installed. FrameForge runs the new version once it restarts.
          </div>
        )}

        {phase === "failed" && (
          <div style={{ fontSize: 12, color: "#f0c040" }}>
            Update failed: {error}. The installed version is unchanged.
          </div>
        )}

        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          {phase === "installed" ? (
            <>
              <button className="btn-secondary" onClick={onDismiss}>Restart later</button>
              <button className="riven-modal-submit" onClick={() => invoke("restart_app")}>
                Restart now
              </button>
            </>
          ) : (
            <>
              <button className="btn-secondary" onClick={onDismiss} disabled={!dismissable}>
                Later
              </button>
              <button className="riven-modal-submit" onClick={install} disabled={phase === "installing"}>
                {phase === "installing" ? "Installing…" : phase === "failed" ? "Try again" : "Install now"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
