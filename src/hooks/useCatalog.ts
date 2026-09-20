import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { TAURI_COMMANDS } from "../constants/tauri";
import type { CatalogItem, RelicDropMap } from "../types/items";

// ─── Singleton ────────────────────────────────────────────────────────────────
// One IPC call per consumer tree, shared by every module that needs the
// catalogue or relic-drop map.  Follows the worldstate.ts pattern exactly:
// module-level state, subscriber set, first subscriber triggers load.

type Snapshot = {
  catalog: CatalogItem[];
  relicDropMap: RelicDropMap;
  loaded: boolean;
};

let current: Snapshot = { catalog: [], relicDropMap: {}, loaded: false };
let inFlight: Promise<void> | null = null;
const subscribers = new Set<(s: Snapshot) => void>();

function publish(next: Snapshot) {
  current = next;
  for (const notify of subscribers) notify(next);
}

function fetchOnce(): Promise<void> {
  inFlight ??= Promise.all([
    invoke<CatalogItem[]>(TAURI_COMMANDS.GET_ALL_ITEMS),
    invoke<RelicDropMap>("get_relic_drops"),
  ])
    .then(([catalog, relicDropMap]) => {
      publish({ catalog, relicDropMap, loaded: true });
    })
    .catch(() => {
      publish({ ...current, loaded: false });
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

/** Re-reads the catalogue after the backend rebuilt it. A fetch already in
 *  flight finishes first so its stale result cannot land after the fresh one. */
export function refreshCatalog(): Promise<void> {
  return (inFlight ?? Promise.resolve()).then(fetchOnce);
}

// ─── Hook ─────────────────────────────────────────────────────────────────────

export interface UseCatalogReturn {
  catalog: CatalogItem[];
  relicDropMap: RelicDropMap;
  loaded: boolean;
  refresh: () => void;
}

export function useCatalog(): UseCatalogReturn {
  const [snapshot, setSnapshot] = useState(current);

  useEffect(() => {
    subscribers.add(setSnapshot);
    if (subscribers.size === 1) {
      fetchOnce();
    } else {
      setSnapshot(current);
    }
    return () => {
      subscribers.delete(setSnapshot);
    };
  }, []);

  const refresh = useCallback(() => { void refreshCatalog(); }, []);

  return { ...snapshot, refresh };
}
