// Arbitration schedule as the backend serves it. All times are unix seconds.
export type ScheduleEntry = {
  start: number;
  end: number;
  node_id: string;
  node: string;
  region: string;
  mission_type: string;
  faction: string;
};

export type Schedule = {
  entries: ScheduleEntry[];
  source: "fresh" | "refreshed" | "refreshing" | "stale" | "fallback";
  warning: string | null;
};

// A zero lead would put the alert at the moment the hour starts, which is the
// one instant it cannot help. The ceiling is one rotation, which keeps the
// alert inside the hour immediately before the one it announces.
export const MIN_LEAD_MINS = 1;
export const MAX_LEAD_MINS = 60;
export const DEFAULT_LEAD_MINS = 10;

// The lead floor above has to span several of these: an alert window exactly
// one tick wide is one delayed timer away from being stepped over.
export const EVAL_INTERVAL_MS = 20_000;

// An hour whose lead window was slept through still alerts, but only while
// this much of it is left. Below that it announces a run nobody can reach in
// time.
export const MIN_REMAINING_SECS = 300;

// One rotation. A fired key carries a start and no end, so pruning needs this
// to work out when the key has aged out of ever matching again.
const ROTATION_SECS = 3600;

export function clampLead(mins: number): number {
  if (!Number.isFinite(mins)) return DEFAULT_LEAD_MINS;
  return Math.min(MAX_LEAD_MINS, Math.max(MIN_LEAD_MINS, Math.round(mins)));
}

// Keyed by occurrence rather than node: a favorited node comes round again
// every few days and has to alert again each time.
export const occurrenceKey = (e: ScheduleEntry) => `${e.start}@${e.node_id}`;
const keyStart = (key: string) => Number(key.split("@")[0]);

// `fired` is the caller's persisted state and `kept` replaces it, which is what
// prunes it: the state stays the size of the horizon rather than growing for as
// long as the app is installed. Keys for `due` are deliberately not in `kept`:
// the caller appends the ones it managed to raise, so an occurrence that could
// not be delivered is retried rather than recorded.
export function dueAlerts(
  entries: readonly ScheduleEntry[],
  favorites: readonly string[],
  leadMins: number,
  fired: readonly string[],
  nowSec: number,
): { due: ScheduleEntry[]; kept: string[] } {
  const leadSecs = clampLead(leadMins) * 60;
  const favorited = new Set(favorites);
  const alreadyFired = new Set(fired);

  const due = entries.filter(e =>
    favorited.has(e.node_id) &&
    !alreadyFired.has(occurrenceKey(e)) &&
    // Once the hour is running the alert is a catch-up for a timer suspended
    // across the lead window, so the time left to join the run decides it
    // instead of the lead.
    (e.start > nowSec
      ? e.start - leadSecs <= nowSec
      : e.end - nowSec > MIN_REMAINING_SECS));

  // A key has to outlive the start it names, because that hour stays alertable
  // while it runs; pruning on start would re-alert a running occurrence every
  // evaluation. A key that parses to no number at all goes immediately.
  return {
    due,
    kept: fired.filter(k => {
      const start = keyStart(k);
      return Number.isFinite(start) && start + ROTATION_SECS > nowSec;
    }),
  };
}

// `raise` says whether the alert was handed over; an occurrence it could not
// raise stays out of the result, so the next pass offers it again. Null means
// nothing moved, which spares the caller a settings write.
export async function runAlertPass(
  entries: readonly ScheduleEntry[],
  favorites: readonly string[],
  leadMins: number,
  fired: readonly string[],
  nowSec: number,
  raise: (entry: ScheduleEntry) => Promise<boolean>,
): Promise<string[] | null> {
  const { due, kept } = dueAlerts(entries, favorites, leadMins, fired, nowSec);

  const raised: string[] = [];
  for (const e of due) if (await raise(e)) raised.push(occurrenceKey(e));

  const next = [...kept, ...raised];
  const unchanged = next.length === fired.length && next.every((k, i) => k === fired[i]);
  return unchanged ? null : next;
}
