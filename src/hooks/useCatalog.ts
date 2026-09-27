import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
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
let listenerStarted = false;
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

// The Rust side rebuilds the catalogue asynchronously in the background (e.g. the
// full re-fetch triggered by a fresh launch after Factory Reset / cache wipe, or the
// daily refresh). Without this, a consumer that mounts and fetches before that rebuild
// finishes is stuck on whatever it captured first (the tiny hardcoded `fallback_items()`
// list, which has zero "Relics" entries) for the rest of the session — see App.tsx's
// own catalog fetch (useInventoryData.ts), which listens for the same event.
function ensureListener() {
  if (listenerStarted) return;
  listenerStarted = true;
  listen(TAURI_EVENTS.CATALOGUE_UPDATED, () => {
    inFlight = null;
    fetchOnce();
  }).catch(() => {
    listenerStarted = false;
  });
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
      ensureListener();
      fetchOnce();
    } else {
      setSnapshot(current);
    }
    return () => {
      subscribers.delete(setSnapshot);
    };
  }, []);

  const refresh = useCallback(() => {
    inFlight = null;
    fetchOnce();
  }, []);

  return { ...snapshot, refresh };
}
