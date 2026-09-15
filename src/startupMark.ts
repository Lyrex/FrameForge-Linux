import { invoke } from "@tauri-apps/api/core";

// js_ms is when the page sent it; the backend stamps when it arrived, so the
// gap between the two is IPC queueing.
export function mark(label: string) {
  invoke("startup_mark", { label: `${label} js_ms=${performance.now().toFixed(0)}` }).catch(() => {});
}
