import type { Access, MasteryState, Opportunity, Stage } from "../types/mastery";

export type MasteryView = "whatnext" | "collection";
export type ResultView = "suggestions" | "relics" | "platinum";
export type ProgressFilter = "all" | Exclude<MasteryState, "mastered">;
export type AvailabilityFilter = "all" | Access;
export type Sort = "mastery" | "name";

export interface MasteryControls {
  view: MasteryView;
  result: ResultView;
  category: string | null;
  progress: ProgressFilter;
  availability: AvailabilityFilter;
  sort: Sort;
}

export const DEFAULT_CONTROLS: MasteryControls = {
  view: "whatnext", result: "suggestions", category: null, progress: "all", availability: "all", sort: "mastery",
};

export const VIEW_OPTIONS: { key: MasteryView; label: string }[] = [
  { key: "whatnext",   label: "What next" },
  { key: "collection", label: "Collection" },
];

export const RESULT_OPTIONS: { key: ResultView; label: string }[] = [
  { key: "suggestions", label: "Suggestions" },
  { key: "relics",      label: "More relics" },
  { key: "platinum",    label: "With platinum" },
];

export const PROGRESS_OPTIONS: { key: ProgressFilter; label: string }[] = [
  { key: "all",     label: "Any progress" },
  { key: "partial", label: "Partial" },
  { key: "missing", label: "Missing" },
  { key: "unknown", label: "Unknown" },
];

export const AVAILABILITY_OPTIONS: { key: AvailabilityFilter; label: string }[] = [
  { key: "all",       label: "Any availability" },
  { key: "available", label: "Available now" },
  { key: "blocked",   label: "Blocked" },
  { key: "unknown",   label: "Unknown access" },
];

export const SORT_OPTIONS: { key: Sort; label: string }[] = [
  { key: "mastery", label: "Remaining mastery" },
  { key: "name",    label: "Name" },
];

function pick<T>(options: { key: T }[], value: unknown, fallback: T): T {
  return options.some(o => o.key === value) ? (value as T) : fallback;
}

export function parseControls(raw: string | null): MasteryControls {
  let stored: Record<string, unknown> = {};
  try { stored = raw ? JSON.parse(raw) : {}; } catch {}
  if (typeof stored !== "object" || stored === null) stored = {};
  return {
    view: pick(VIEW_OPTIONS, stored.view, DEFAULT_CONTROLS.view),
    result: pick(RESULT_OPTIONS, stored.result, DEFAULT_CONTROLS.result),
    // The valid categories come with the overview, so the view checks this one.
    category: typeof stored.category === "string" ? stored.category : null,
    progress: pick(PROGRESS_OPTIONS, stored.progress, DEFAULT_CONTROLS.progress),
    availability: pick(AVAILABILITY_OPTIONS, stored.availability, DEFAULT_CONTROLS.availability),
    sort: pick(SORT_OPTIONS, stored.sort, DEFAULT_CONTROLS.sort),
  };
}

export const STAGE_ORDER: readonly Stage[] = ["level_claim", "acquire"];

export const STAGE_LABELS: Record<Stage, string> = {
  level_claim: "Level, claim or spend",
  acquire: "Acquire",
};

export function visibleOpportunities(list: Opportunity[], controls: MasteryControls, search: string): Opportunity[] {
  const q = search.trim().toLowerCase();
  const visible = list.filter(o =>
    (controls.category == null || o.category === controls.category)
    && (controls.progress === "all" || o.state === controls.progress)
    && (controls.availability === "all" || o.access === controls.availability)
    && (!q || o.name.toLowerCase().includes(q)));
  if (controls.sort !== "name") return visible;
  return visible.sort((a, b) =>
    STAGE_ORDER.indexOf(a.stage) - STAGE_ORDER.indexOf(b.stage) || a.name.localeCompare(b.name));
}

export function remainingText(remaining: number | null): string {
  return remaining == null ? "Unknown" : `+${remaining.toLocaleString("en-US")}`;
}

/** Uses the owned copy's level. A forma'd copy can sit below the mastery credit already earned. */
export function actionText(o: Opportunity): string {
  switch (o.action) {
    case "level": return o.owned_level == null ? `Level to R${o.cap}` : `Level R${o.owned_level} → R${o.cap}`;
    case "claim": return "Claim from Foundry";
    case "spend": {
      const s = o.spend;
      return s ? `Spend ${s.points.toLocaleString("en-US")} points for ${s.ranks} rank${s.ranks === 1 ? "" : "s"}` : "Spend";
    }
    case "buy": {
      const first = o.vendors[0];
      return first ? `Buy ${first.blueprint ? "blueprint " : ""}from ${first.syndicate}` : "Buy";
    }
  }
}

export function readyText(completionMs: number, nowMs: number): string {
  const left = completionMs - nowMs;
  if (left <= 0) return "ready";
  const minutes = Math.max(1, Math.ceil(left / 60_000));
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);
  if (days > 0) return `ready in ${days}d ${hours % 24}h`;
  if (hours > 0) return `ready in ${hours}h ${minutes % 60}m`;
  return `ready in ${minutes}m`;
}

export function detailText(o: Opportunity, nowMs: number, readyAt?: string): string {
  const parts: string[] = [];
  if (o.owned) parts.push("Owned copy");
  if (o.build_completion_ms != null) {
    parts.push(`Build ${readyText(o.build_completion_ms, nowMs)}${readyAt ? ` (${readyAt})` : ""}`);
  }
  for (const v of o.vendors) parts.push(`${v.syndicate}${v.tier ? `, ${v.tier}` : ""}${v.blueprint ? " (blueprint)" : ""}`);
  if (o.spend) {
    parts.push(`+${o.spend.mastery.toLocaleString("en-US")} mastery`);
    for (const t of o.spend.tracks) parts.push(`${t.track} R${t.from} → R${t.to}`);
  }
  return parts.join(" · ");
}
