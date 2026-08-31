import { useState } from "react";
import { checkForUpdate, type UpdateAvailable } from "./updater";
import { checkMessage, errorText, type UpdateCheck } from "./updateFlow";

interface Props {
  onUpdateFound: (update: UpdateAvailable) => void;
}

export default function UpdateCheckRow({ onUpdateFound }: Props) {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");

  async function run() {
    setBusy(true);
    setMsg("");
    let result: UpdateCheck;
    try {
      const update = await checkForUpdate();
      result = update ? { kind: "available", update } : { kind: "current" };
    } catch (e) {
      result = { kind: "failed", message: errorText(e) };
    }
    setBusy(false);
    setMsg(checkMessage(result));
    if (result.kind === "available") onUpdateFound(result.update);
  }

  return (
    <>
      <div className="settings-row">
        <div className="settings-row-info">
          <span className="settings-row-label">Updates</span>
          <span className="settings-row-desc">
            Nothing is downloaded or installed until you confirm it.
          </span>
        </div>
        <button className="btn-secondary" onClick={run} disabled={busy}>
          {busy ? "Checking…" : "Check for updates"}
        </button>
      </div>
      {msg && <div className="settings-msg">{msg}</div>}
    </>
  );
}
