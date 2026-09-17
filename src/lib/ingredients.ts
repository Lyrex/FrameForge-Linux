import type { CraftPlan, Requirement } from "../types/mastery";
import { CREDITS_PATH, FORMA_PATH, INGREDIENT_STATE_LABELS } from "../constants/ingredients.ts";

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
  return [{ unique_name: CREDITS_PATH, name: "Credits", image_name: null, category: null, needed, owned: plan.credit_balance ?? from_stock, from_stock, short, state }, ...plan.requirements];
}

/**
 * Only parts get an icon, because a resource is what the player farms while the icon row reads
 * the recipe. Forma is the one resource a Level row needs on its own. An ingredient the catalogue
 * does not list stays, since nothing says it is a resource.
 */
export function iconLines(plan: CraftPlan): Requirement[] {
  return plan.requirements.filter(r => r.unique_name === FORMA_PATH || r.category !== "Resources");
}

export function ingredientTitle(line: Requirement): string {
  const owned = line.reusable && line.owned > 0 ? "∞" : line.owned.toLocaleString("en-US");
  const counts = line.unique_name === CREDITS_PATH && line.needed === 0 ? [] : [`Available: ${owned} / ${line.needed.toLocaleString("en-US")}`];
  const label = INGREDIENT_STATE_LABELS[line.state];
  return [line.name, ...counts, ...(label ? [label] : [])].join("\n");
}
