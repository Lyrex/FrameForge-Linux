import { formatCount as n } from "../lib/formatters.ts";
import { RESULT_OPTIONS, shownControls, visibleOpportunities, visiblePurchases, type MasteryControls, type ResultView } from "./suggestions.ts";
import type { MasteryPlan, MasteryTotal, Opportunity, PlanEvaluation } from "../types/mastery";

/** Legendary 5 is MR 35 today. The rest is headroom for ranks the game adds later. */
export const TARGET_RANK_MAX = 40;

export function planView(raw: string): ResultView {
  return RESULT_OPTIONS.some(o => o.key === raw) ? (raw as ResultView) : "suggestions";
}

export function gainOf(o: Opportunity): number | null {
  return o.spend?.mastery ?? o.remaining_mastery;
}

export function candidates(list: Opportunity[], controls: MasteryControls, view: ResultView, search: string): Opportunity[] {
  const scoped = shownControls({ ...controls, result: view });
  return view === "platinum" ? visiblePurchases(list, scoped, search) : visibleOpportunities(list, scoped, search);
}

export function snapshotIntrinsicTargets(plan: MasteryPlan, opportunities: Opportunity[]): Record<string, Record<string, number>> {
  const targets: Record<string, Record<string, number>> = {};
  for (const path of plan.selections) {
    const saved = plan.intrinsic_targets?.[path];
    const spend = opportunities.find(o => o.unique_name === path)?.spend;
    if (saved) targets[path] = saved;
    else if (spend) targets[path] = Object.fromEntries(spend.tracks.map(t => [t.track, t.to]));
  }
  return targets;
}

export function addToPlan(selections: string[], opportunity: Opportunity): string[] {
  return [...new Set([...selections, ...opportunity.craft?.level_first?.map(step => step.unique_name) ?? [], opportunity.unique_name])];
}

// ponytail: takes candidates greedily in view order. The smallest covering set would need a search.
export function prefill(candidates: Opportunity[], gap: number): string[] {
  let selections: string[] = [];
  let covered = 0;
  for (const o of candidates) {
    if (covered >= gap) break;
    // Prefill skips a row with no route because the plan could not say how to get it.
    if (o.stage === "unsourced") continue;
    if (selections.includes(o.unique_name)) continue;
    for (const step of o.craft?.level_first ?? []) {
      if (!selections.includes(step.unique_name)) covered += step.gain;
    }
    selections = addToPlan(selections, o);
    covered += gainOf(o) ?? 0;
  }
  return selections;
}

export function addable(candidates: Opportunity[], plan: MasteryPlan, search: string): Opportunity[] {
  const q = search.trim().toLowerCase();
  const selected = new Set(plan.selections);
  return candidates.filter(o => !selected.has(o.unique_name) && (!q || o.name.toLowerCase().includes(q)));
}

export function moved(selections: string[], index: number, by: -1 | 1): string[] {
  const to = index + by;
  if (to < 0 || to >= selections.length) return selections;
  const next = [...selections];
  [next[index], next[to]] = [next[to], next[index]];
  return next;
}

export function totalText(total: MasteryTotal): string {
  return total.exact == null ? `at least ${n(total.lower)}` : n(total.exact);
}

export function rankText(total: MasteryTotal): string {
  return total.exact == null ? `at least MR ${total.rank}` : `MR ${total.rank}`;
}

export function summaryText(e: PlanEvaluation, target: number): string {
  if (e.total == null || e.projected == null || e.gap == null) return "Mastery Rank not observed yet, so the gap and projection are unknown.";
  const unknown = e.unknown_gains > 0 ? ` and ${e.unknown_gains} unknown` : "";
  const gap = e.gap === 0 ? `MR ${target} is reached` : `MR ${target} needs ${n(e.gap)} more`;
  return `Total ${totalText(e.total)} · ${gap} · plan adds ${n(e.gains)}${unknown} · projected ${rankText(e.projected)}`;
}

export function rangeText(e: PlanEvaluation): string {
  if (e.total == null || e.projected == null) return "";
  const total = e.total.exact == null ? `Total between ${n(e.total.lower)} and ${n(e.total.upper)}` : `Total ${n(e.total.exact)} exactly`;
  const projected = e.projected.exact == null
    ? `projected between ${n(e.projected.lower)} and ${n(e.projected.upper)}, MR ${e.projected.rank} to ${e.projected.rank_upper}`
    : `projected ${n(e.projected.exact)}, MR ${e.projected.rank}`;
  return `${total} · ${projected} · target MR needs ${n(e.target_xp)}`;
}
