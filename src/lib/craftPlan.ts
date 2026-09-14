import type { CraftPlan, Requirement } from "../types/mastery";

export type CraftRow = Requirement & { crafts: number };

export function craftRows(plan: CraftPlan): CraftRow[] {
  const crafts = new Map(plan.builds.map(b => [b.unique_name, b.crafts]));
  return plan.requirements
    .map(r => ({ ...r, crafts: crafts.get(r.unique_name) ?? 0 }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

const covered = (plan: CraftPlan) => plan.requirements.every(r => r.short === 0);

export const craftableNow = (plan: CraftPlan) => plan.builds.length === 0 && covered(plan);

export const buildable = (plan: CraftPlan) => plan.builds.length > 0 && covered(plan);
