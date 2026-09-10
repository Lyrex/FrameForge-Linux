import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { check } from "@tauri-apps/plugin-updater";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { UpdateAvailable } from "./updater";
import { advance, errorText, noProgress, type DownloadProgress } from "./updateFlow";

interface Props {
  update: UpdateAvailable;
  onDismiss: () => void;
}

type Phase = "prompt" | "installing" | "installed" | "failed";

export default function UpdateDialog({ update, onDismiss }: Props) {
  const [phase, setPhase] = useState<Phase>("prompt");
  const [progress, setProgress] = useState<DownloadProgress>(noProgress);
  const [error, setError] = useState("");

  const dismissable = phase !== "installing";

  // Keyboard focus moves into the dialog so Escape is handled here and stops
  // before any overlay underneath, which listens on the window, sees it too.
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => dialog.current?.focus(), []);

  // Rendered once: every download chunk re-renders the dialog, and the
  // Markdown component parses on each render.
  const notes = useMemo(() => update.notes && (
    <div className="release-notes">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          // A link that navigated the webview would strand the user: it has no back button.
          a: ({ href, children }) => (
            <a href={href} onClick={e => { e.preventDefault(); if (href) openUrl(href).catch(console.error); }}>
              {children}
            </a>
          ),
        }}
      >
        {update.notes}
      </Markdown>
    </div>
  ), [update.notes]);

  async function install() {
    setPhase("installing");
    setProgress(noProgress);
    try {
      // The launch check reports the new version but keeps no handle on it,
      // so the manifest is fetched again to get one that can be downloaded.
      const handle = await check();
      if (!handle) throw new Error("the release is no longer published");
      await handle.downloadAndInstall(e => setProgress(p => advance(p, e)));
      // Only Linux gets this far. On Windows the plugin hands the downloaded
      // package to the installer and exits, and the installer starts the new
      // version itself, so there is no restart to offer.
      setPhase("installed");
    } catch (e) {
      setError(errorText(e));
      setPhase("failed");
    }
  }

  return (
    <div className="ff-modal-overlay" onClick={() => dismissable && onDismiss()}>
      <div
        className="ff-modal"
        ref={dialog}
        tabIndex={-1}
        onClick={e => e.stopPropagation()}
        onKeyDown={e => {
          if (e.key !== "Escape" || !dismissable) return;
          e.stopPropagation();
          onDismiss();
        }}
      >
        <div className="riven-modal-title">
          FrameForge {update.version} is available
        </div>

        <div style={{ fontSize: 12, color: "var(--muted)" }}>
          You are running {update.currentVersion}.
        </div>

        {notes}

        {phase === "installing" && (
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <div style={{ fontSize: 12, color: "var(--muted)" }}>
              {progress.finished ? "Installing…" : "Downloading…"}
            </div>
            {progress.total !== null && !progress.finished
              ? <progress value={progress.received} max={progress.total} />
              : <progress />}
          </div>
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
                {phase === "failed" ? "Try again" : "Install now"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
