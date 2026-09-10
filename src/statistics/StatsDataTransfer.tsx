import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type ImportCounts = {
  trades_added: number;
  trades_skipped: number;
  snapshots_added: number;
  snapshots_skipped: number;
  tracked_added: number;
  tracked_skipped: number;
  runs_added: number;
  runs_skipped: number;
};

/** Both commands return null when the user dismisses the file dialog, which
 *  must not read as a failure. */
export default function StatsDataTransfer() {
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [msg, setMsg] = useState("");

  async function run(which: "export" | "import") {
    setBusy(which);
    setMsg("");
    try {
      if (which === "export") {
        const path = await invoke<string | null>("export_stats");
        if (path) setMsg(`Exported to ${path}`);
      } else {
        const c = await invoke<ImportCounts | null>("import_stats");
        if (c) setMsg(
          `Imported — trades ${c.trades_added} added, ${c.trades_skipped} already present · ` +
          `snapshots ${c.snapshots_added} added, ${c.snapshots_skipped} already present · ` +
          `tracked items ${c.tracked_added} added, ${c.tracked_skipped} already present · ` +
          `arbitration runs ${c.runs_added} added, ${c.runs_skipped} already present.`
        );
      }
    } catch (e) {
      setMsg(`Error: ${e}`);
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="settings-section">
      <div className="settings-section-title">Statistics Backup</div>
      <div className="settings-row">
        <div className="settings-row-info">
          <span className="settings-row-label">Export</span>
          <span className="settings-row-desc">Save trade history, item snapshots, tracked items and arbitration runs to a JSON file.</span>
        </div>
        <button className="btn-secondary" onClick={() => run("export")} disabled={busy !== null}>
          {busy === "export" ? "Exporting…" : "Export"}
        </button>
      </div>
      <div className="settings-row">
        <div className="settings-row-info">
          <span className="settings-row-label">Import</span>
          <span className="settings-row-desc">Merge a previously exported file. Existing entries are kept; nothing is deleted.</span>
        </div>
        <button className="btn-secondary" onClick={() => run("import")} disabled={busy !== null}>
          {busy === "import" ? "Importing…" : "Import"}
        </button>
      </div>
      {msg && <div className="settings-msg">{msg}</div>}
    </div>
  );
}
