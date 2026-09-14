import { RELIC_SUGGESTION_THRESHOLD } from "../constants/relics.ts";
import type { Access, Coverage, MasteryState, Opportunity, Stage } from "../types/mastery";

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
  /** Keeps relic routes out of Suggestions, while More relics still lists them. */
  hideRelics: boolean;
}

export const DEFAULT_CONTROLS: MasteryControls = {
  view: "whatnext", result: "suggestions", category: null, progress: "all", availability: "all", sort: "mastery", hideRelics: false,
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
    hideRelics: stored.hideRelics === true,
  };
}

export const STAGE_ORDER: readonly Stage[] = ["level_claim", "craft", "acquire"];

export const STAGE_LABELS: Record<Stage, string> = {
  level_claim: "Level, claim or spend",
  craft: "Craft",
  acquire: "Acquire",
};

export const RELIC_GROUP_ORDER: readonly Coverage["kind"][] = ["complete", "partial", "unknown"];

export const RELIC_GROUP_LABELS: Record<Coverage["kind"], string> = {
  complete: "By completion chance",
  partial: "Partial coverage",
  unknown: "Chance unknown",
};

/** Classifies on the unrounded probability, so exactly the threshold stays in More relics. */
export function inSuggestions(o: Opportunity): boolean {
  return o.relic == null || (o.relic.coverage.kind === "complete" && o.relic.coverage.probability > RELIC_SUGGESTION_THRESHOLD);
}

function chance(o: Opportunity): number {
  return o.relic?.coverage.kind === "complete" ? o.relic.coverage.probability : -1;
}

export function visibleOpportunities(list: Opportunity[], controls: MasteryControls, search: string): Opportunity[] {
  const q = search.trim().toLowerCase();
  const visible = list.filter(o =>
    (controls.result === "relics" ? o.relic != null && !inSuggestions(o) : inSuggestions(o) && !(controls.hideRelics && o.relic != null))
    && (controls.category == null || o.category === controls.category)
    && (controls.progress === "all" || o.state === controls.progress)
    && (controls.availability === "all" || o.access === controls.availability)
    && (!q || o.name.toLowerCase().includes(q)));
  if (controls.result === "relics") {
    return visible.sort((a, b) =>
      RELIC_GROUP_ORDER.indexOf(a.relic!.coverage.kind) - RELIC_GROUP_ORDER.indexOf(b.relic!.coverage.kind)
      || chance(b) - chance(a)
      || (controls.sort === "name" ? a.name.localeCompare(b.name) : 0));
  }
  if (controls.sort !== "name") return visible;
  return visible.sort((a, b) =>
    STAGE_ORDER.indexOf(a.stage) - STAGE_ORDER.indexOf(b.stage) || a.name.localeCompare(b.name));
}

export function chanceText(coverage: Coverage): string {
  switch (coverage.kind) {
    case "complete": return `${(coverage.probability * 100).toFixed(coverage.probability < 0.01 && coverage.probability > 0 ? 1 : 0)}%`;
    case "partial": return "Partial";
    case "unknown": return "Unknown";
  }
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
    case "craft": return o.access === "available" ? "Craft now" : "Craft";
    case "build": {
      const n = o.craft?.builds.length ?? 0;
      return `Build ${n} ${n === 1 ? "part" : "parts"}, then craft`;
    }
    case "farm": {
      const relicParts = new Set(o.relic?.parts.map(p => p.unique_name));
      const n = o.craft?.requirements.filter(r => r.short > 0 && !relicParts.has(r.unique_name)).length ?? 0;
      const items = `${n} ${n === 1 ? "item" : "items"}`;
      return o.relic ? (n > 0 ? `Farm relics + ${items}` : "Farm relics") : `Farm ${items}`;
    }
    case "buy": {
      const first = o.vendors[0];
      return first ? `Buy ${first.blueprint ? "blueprint " : ""}from ${first.syndicate}` : "Buy";
    }
    case "complete": return "Complete node";
    case "unlock": return "Unlock junction";
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
  if (o.node) parts.push(o.node.planet);
  if (o.owned) parts.push("Owned copy");
  if (o.build_completion_ms != null) {
    parts.push(`Build ${readyText(o.build_completion_ms, nowMs)}${readyAt ? ` (${readyAt})` : ""}`);
  }
  for (const v of o.vendors) parts.push(`${v.syndicate}${v.tier ? `, ${v.tier}` : ""}${v.blueprint ? " (blueprint)" : ""}`);
  if (o.spend) {
    parts.push(`+${o.spend.mastery.toLocaleString("en-US")} mastery`);
    for (const t of o.spend.tracks) parts.push(`${t.track} R${t.from} → R${t.to}`);
  }
  if (o.relic) {
    const { coverage } = o.relic;
    if (coverage.kind === "partial") {
      if (coverage.missing.length) parts.push(`No relic for ${coverage.missing.join(", ")}`);
      if (coverage.short.length) parts.push(`Too few relics for ${coverage.short.join(", ")}`);
    }
    for (const p of o.relic.parts) {
      if (p.relics.length === 0) continue;
      const count = p.needed > 1 ? ` ×${p.needed}` : "";
      parts.push(`${p.name}${count} from ${p.relics.map(r => `${r.name} ×${r.count}`).join(", ")}`);
    }
  }
  if (o.craft) {
    parts.push(o.craft.credits == null ? "Credits unknown" : `Credits ${o.craft.credits.toLocaleString("en-US")}`);
    if (o.craft.builds.length) parts.push(`Build ${o.craft.builds.map(b => b.crafts > 1 ? `${b.name} ×${b.crafts}` : b.name).join(", ")}`);
    const relicParts = new Set(o.relic?.parts.map(p => p.unique_name));
    const short = o.craft.requirements.filter(r => r.short > 0 && !relicParts.has(r.unique_name));
    if (short.length) parts.push(`Short ${short.map(r => `${r.name} ×${r.short.toLocaleString("en-US")}`).join(", ")}`);
  }
  return parts.join(" · ");
}
