import type { DownloadEvent } from "@tauri-apps/plugin-updater";
import type { UpdateAvailable, UpdateStatus } from "./updater";

export type UpdateCheck =
  | { kind: "available"; update: UpdateAvailable }
  | { kind: "current" }
  | { kind: "unmanaged" }
  | { kind: "failed"; message: string };

/**
 * Takes the check rather than calling it, so the mapping the user sees is the
 * one under test.
 */
export async function runCheck(
  check: () => Promise<UpdateStatus>,
): Promise<UpdateCheck> {
  try {
    const status = await check();
    if (!status.selfUpdates) return { kind: "unmanaged" };
    return status.update
      ? { kind: "available", update: status.update }
      : { kind: "current" };
  } catch (e) {
    return { kind: "failed", message: errorText(e) };
  }
}

export function checkMessage(result: UpdateCheck): string {
  switch (result.kind) {
    case "available":
      return `Version ${result.update.version} is available.`;
    case "current":
      return "FrameForge is up to date.";
    case "unmanaged":
      return "This install updates through your package manager.";
    case "failed":
      return `Could not check for updates: ${result.message}`;
  }
}

/** Tauri rejects with a plain string, the browser with an `Error`. */
export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export type DownloadProgress = {
  received: number;
  /** `null` when the server announced no length. */
  total: number | null;
  /** The bytes are down; what remains is the install step, of unknown length. */
  finished: boolean;
};

export const noProgress: DownloadProgress = { received: 0, total: null, finished: false };

export function advance(p: DownloadProgress, e: DownloadEvent): DownloadProgress {
  switch (e.event) {
    case "Started":
      return { received: 0, total: e.data.contentLength ?? null, finished: false };
    case "Progress":
      return { ...p, received: p.received + e.data.chunkLength };
    case "Finished":
      return { ...p, finished: true };
  }
}
