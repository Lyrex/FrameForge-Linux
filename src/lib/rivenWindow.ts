import { invoke } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalSize } from "@tauri-apps/api/window";
import { overlayScale } from "./uiScale";

// ── Riven overlay — module-level window management ────────────────────────────
// Stored OUTSIDE React so StrictMode remounts don't destroy/recreate the window.
let _rivenWin: WebviewWindow | null = null;
let _rivenRollCount = 0;
let _rivenLastTriggerMs = 0;
let _rivenManualTrigger: (() => void) | null = null;

export function checkRivenNow() { _rivenManualTrigger?.(); }

export function setRivenManualTrigger(fn: (() => void) | null) {
  _rivenManualTrigger = fn;
}

export function incrementRivenRollCount() {
  _rivenRollCount++;
  return _rivenRollCount;
}

export function getRivenRollCount() {
  return _rivenRollCount;
}

export function getRivenLastTriggerMs() {
  return _rivenLastTriggerMs;
}

export function setRivenLastTriggerMs(ms: number) {
  _rivenLastTriggerMs = ms;
}

export async function resizeRivenForScale() {
  const win = _rivenWin;
  if (!win) return;
  try {
    const factor = await win.scaleFactor();
    const cur = (await win.innerSize()).toLogical(factor);
    await win.setSize(new LogicalSize(Math.round(300 * overlayScale()), cur.height));
  } catch {}
}

export function rivenWinHide(reason = "rivenWinHide") {
  const win = _rivenWin;
  if (!win) { return; }
  invoke("ocr_riven_log_error", { error: `[HIDE] ${reason}` }).catch(() => {});
  _rivenWin = null;
  win.close().catch(() => {});
}

export async function ensureRivenWindow(wx: number, wy: number, wh: number): Promise<{ win: WebviewWindow; fresh: boolean } | null> {
  // 1. Existing valid handle
  if (_rivenWin) return { win: _rivenWin, fresh: false };

  // 2. Window exists but JS lost reference (HMR, page reload)
  const existing = await WebviewWindow.getByLabel("riven-overlay").catch(() => null);
  if (existing) {
    _rivenWin = existing;
    _rivenWin.once("tauri://destroyed", () => { _rivenWin = null; });
    return { win: _rivenWin, fresh: false };
  }

  // 3. Create fresh at correct position — shows immediately
  try {
    _rivenWin = new WebviewWindow("riven-overlay", {
      url: `index.html#rivenoverlay`,
      title: "FrameForge Riven",
      transparent: true, decorations: false,
      alwaysOnTop: true, skipTaskbar: true,
      resizable: false, focus: false,
      x: wx + 10, y: wy + Math.round(wh * 0.20),
      width: Math.round(300 * overlayScale()), height: Math.round(wh * 0.60),
    });
    _rivenWin.once("tauri://destroyed", () => { _rivenWin = null; });
    return { win: _rivenWin, fresh: true };
  } catch {
    _rivenWin = null;
    return null;
  }
}
