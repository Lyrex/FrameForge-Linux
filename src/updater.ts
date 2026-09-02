import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const UPDATE_AVAILABLE_EVENT = "update-available";

export type UpdateAvailable = {
  version: string;
  currentVersion: string;
  notes: string | null;
};

/**
 * Fires once per launch, only when a newer release exists. Every other outcome
 * is silent, so there is no failure event to handle.
 *
 * The event can be emitted before the webview subscribes, and Tauri drops an
 * event with no listener, so pair this with `pendingUpdate` on mount.
 */
export function onUpdateAvailable(
  handler: (update: UpdateAvailable) => void,
): Promise<UnlistenFn> {
  return listen<UpdateAvailable>(UPDATE_AVAILABLE_EVENT, (e) => handler(e.payload));
}

export type UpdateStatus = {
  update: UpdateAvailable | null;
  /** False for a `.deb` or `.rpm` install, which updates through its package
   * manager rather than from the release manifest. */
  selfUpdates: boolean;
};

/**
 * The same check on demand, rejecting when the check itself fails. To download
 * and apply an update, use `check()` from `@tauri-apps/plugin-updater` instead
 * — it hands back an installable handle, which this does not.
 */
export function checkForUpdate(): Promise<UpdateStatus> {
  return invoke<UpdateStatus>("check_for_update");
}

/** What the launch check found, for a webview that booted after it ran. */
export function pendingUpdate(): Promise<UpdateAvailable | null> {
  return invoke<UpdateAvailable | null>("pending_update");
}
