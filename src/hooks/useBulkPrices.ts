import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { TAURI_COMMANDS } from "../constants/tauri";

// ─── Singleton ────────────────────────────────────────────────────────────────
// One IPC call for every consumer needing the daily bulk price map
// (lowercase display name → plat). Backend keeps it fresh; consumers
// re-fetch via refresh() when caches are refreshed.

type Snapshot = {
  bulkPrices: Map<string, number>;
  loaded: boolean;
};

let current: Snapshot = { bulkPrices: new Map(), loaded: false };
let inFlight: Promise<void> | null = null;
const subscribers = new Set<(s: Snapshot) => void>();

function publish(next: Snapshot) {
  current = next;
  for (const notify of subscribers) notify(next);
}

function fetchOnce(): Promise<void> {
  inFlight ??= invoke<Record<string, number>>(TAURI_COMMANDS.GET_BULK_PRICES)
    .then(raw => {
      const prices = new Map<string, number>();
      for (const [name, price] of Object.entries(raw ?? {})) prices.set(name, price);
      publish({ bulkPrices: prices, loaded: true });
    })
    .catch(() => {
      publish({ ...current, loaded: false });
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

// ─── Hook ─────────────────────────────────────────────────────────────────────

export interface UseBulkPricesReturn {
  bulkPrices: Map<string, number>;
  loaded: boolean;
  refresh: () => void;
}

export function useBulkPrices(): UseBulkPricesReturn {
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

  const refresh = useCallback(() => {
    // Skip if a fetch is already running (mount triggers one via fetchOnce).
    if (inFlight) return;
    fetchOnce();
  }, []);

  return { ...snapshot, refresh };
}
