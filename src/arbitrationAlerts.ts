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

export function clampLead(mins: number): number {
  if (!Number.isFinite(mins)) return DEFAULT_LEAD_MINS;
  return Math.min(MAX_LEAD_MINS, Math.max(MIN_LEAD_MINS, Math.round(mins)));
}

// Keyed by occurrence rather than node: a favorited node comes round again
// every few days and has to alert again each time.
const occurrenceKey = (e: ScheduleEntry) => `${e.start}@${e.node_id}`;
const keyStart = (key: string) => Number(key.split("@")[0]);

// `fired` is the caller's persisted state; the returned list replaces it. That
// replacement is what prunes, so the state stays the size of the horizon
// rather than growing for as long as the app is installed.
export function dueAlerts(
  entries: readonly ScheduleEntry[],
  favorites: readonly string[],
  leadMins: number,
  fired: readonly string[],
  nowSec: number,
): { due: ScheduleEntry[]; fired: string[] } {
  const leadSecs = clampLead(leadMins) * 60;
  const favorited = new Set(favorites);
  const alreadyFired = new Set(fired);

  const due = entries.filter(e =>
    favorited.has(e.node_id) &&
    // An hour already under way is never announced. Late is useless, and after
    // a restart it would arrive for a mission the user may already be running.
    e.start > nowSec &&
    e.start - leadSecs <= nowSec &&
    !alreadyFired.has(occurrenceKey(e)));

  // A key whose hour has begun can never match again. An unparsable one — a
  // hand-edited settings file — reads as NaN and drops out the same way.
  const kept = fired.filter(k => keyStart(k) > nowSec);
  return { due, fired: [...kept, ...due.map(occurrenceKey)] };
}
