import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Schedule } from "./arbitrationAlerts";

// One poll per window, shared by the browser and the alert loop. The backend
// command re-reads and re-parses the whole multi-year feed on every call, so a
// timer per consumer is not free the way a cache hit sounds like it should be.
//
// Hourly, because that is how often the rotation changes; the entries already
// reach three days ahead, so a schedule fetched fifty minutes ago still holds
// every hour anyone is about to be alerted about.
const POLL_MS = 60 * 60 * 1000;

// Until one fetch has landed there is nothing to alert from at all, and an
// hour's wait for the next attempt is long enough to walk past an occurrence
// entirely. Backs off so a feed that is down all afternoon is not hammered.
const RETRY_MS = [15_000, 60_000, 300_000];

type Snapshot = { schedule: Schedule | null; error: string };

let current: Snapshot = { schedule: null, error: "" };
let inFlight: Promise<void> | null = null;
let timer: ReturnType<typeof setInterval> | null = null;
let retry: ReturnType<typeof setTimeout> | null = null;
let failures = 0;
const subscribers = new Set<(s: Snapshot) => void>();

function publish(next: Snapshot) {
  current = next;
  for (const notify of subscribers) notify(next);
}

function clearRetry() {
  if (retry) { clearTimeout(retry); retry = null; }
}

function fetchOnce(): Promise<void> {
  inFlight ??= invoke<Schedule>("fetch_arbitration_schedule")
    .then(s => { failures = 0; clearRetry(); publish({ schedule: s, error: "" }); })
    // The last good schedule is published alongside the error rather than
    // cleared: its entries stay valid for days, so the alert loop keeps
    // working through a failed refresh and the browser can show what it has.
    .catch(e => { publish({ ...current, error: String(e) }); scheduleRetry(); })
    .finally(() => { inFlight = null; });
  return inFlight;
}

// Only while nothing usable is cached. Once a schedule is in hand the hourly
// poll is soon enough, because those entries already cover every hour anyone
// is about to be alerted about.
function scheduleRetry() {
  if (current.schedule || retry || subscribers.size === 0) return;
  retry = setTimeout(() => {
    retry = null;
    void fetchOnce();
  }, RETRY_MS[Math.min(failures++, RETRY_MS.length - 1)]);
}

// `enabled` is false for a consumer that wants the schedule only if someone
// else is already paying for it: the alert loop must not start an hourly fetch
// for a user who has never starred a node.
export function useArbitrationSchedule(enabled: boolean): Snapshot & { refresh: () => void } {
  const [snapshot, setSnapshot] = useState(current);

  useEffect(() => {
    if (!enabled) return;
    subscribers.add(setSnapshot);
    if (subscribers.size === 1) {
      fetchOnce();
      timer = setInterval(fetchOnce, POLL_MS);
    } else {
      setSnapshot(current);
    }
    return () => {
      subscribers.delete(setSnapshot);
      if (subscribers.size > 0) return;
      if (timer) { clearInterval(timer); timer = null; }
      clearRetry();
      // The next subscriber starts its own run at whatever the feed is doing
      // then, not at the back-off this one ended on.
      failures = 0;
    };
  }, [enabled]);

  return { ...snapshot, refresh: fetchOnce };
}
