import type { UpdateAvailable } from "./updater";

export type UpdateCheck =
  | { kind: "available"; update: UpdateAvailable }
  | { kind: "current" }
  | { kind: "failed"; message: string };

/**
 * A check the user asked for has to report every outcome, including the two
 * the launch check swallows: already current, and the check itself failing.
 */
export function checkMessage(result: UpdateCheck): string {
  switch (result.kind) {
    case "available":
      return `Version ${result.update.version} is available.`;
    case "current":
      return "FrameForge is up to date.";
    case "failed":
      return `Could not check for updates: ${result.message}`;
  }
}

/** Tauri rejects with a plain string, the browser with an `Error`. */
export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
