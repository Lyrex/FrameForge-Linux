import type { CraftPlan, Requirement } from "../types/mastery";
import { CREDITS_PATH, INGREDIENT_STATE_LABELS } from "../constants/ingredients.ts";

/**
 * Credits are not a requirement line on the wire, because a credit shortfall only blocks the craft
 * and never sends the player farming, so their state comes from the plan's cost and shortfall.
 * A zero cost shows nothing, since levelling costs nothing. An unknown cost reads missing with no
 * counts, because a partial sum would pass for the price.
 */
export function ingredients(plan: CraftPlan): Requirement[] {
  if (plan.credits === 0) return plan.requirements;
  const needed = plan.credits ?? 0;
  const short = plan.credits == null ? 0 : plan.credits_short;
  const from_stock = needed - short;
  const state = plan.credits == null ? "missing" : short === 0 ? "owned" : from_stock > 0 ? "partial" : "missing";
  return [{ unique_name: CREDITS_PATH, name: "Credits", image_name: null, needed, from_stock, short, state }, ...plan.requirements];
}

export function ingredientTitle(line: Requirement): string {
  const counts = line.unique_name === CREDITS_PATH && line.needed === 0
    ? "Cost unknown"
    : `${line.from_stock.toLocaleString("en-US")} / ${line.needed.toLocaleString("en-US")}`;
  const label = INGREDIENT_STATE_LABELS[line.state];
  return [line.name, counts, ...(label ? [label] : [])].join("\n");
}
