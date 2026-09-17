import { invoke } from "@tauri-apps/api/core";
import { TAURI_COMMANDS } from "./constants/tauri";

// js_ms is when the page sent it; the backend stamps when it arrived, so the
// gap between the two is IPC queueing.
export function mark(label: string) {
  invoke(TAURI_COMMANDS.STARTUP_MARK, { label: `${label} js_ms=${performance.now().toFixed(0)}` }).catch(() => {});
}
