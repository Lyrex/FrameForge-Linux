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
 */
export function onUpdateAvailable(
  handler: (update: UpdateAvailable) => void,
): Promise<UnlistenFn> {
  return listen<UpdateAvailable>(UPDATE_AVAILABLE_EVENT, (e) => handler(e.payload));
}

/**
 * The same check on demand, resolving to null when there is nothing to
 * install. To download and apply an update, use `check()` from
 * `@tauri-apps/plugin-updater` instead — it hands back an installable handle,
 * which this does not.
 */
export function checkForUpdate(): Promise<UpdateAvailable | null> {
  return invoke<UpdateAvailable | null>("check_for_update");
}
