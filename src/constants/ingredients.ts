import type { IngredientState } from "../types/mastery";

/** The inventory cache's key for the credit balance. */
export const CREDITS_PATH = "/_currency/Credits";

export const FORMA_PATH = "/Lotus/Types/Items/MiscItems/Forma";

/** The catalogue's shared art for a blueprint that has none of its own. */
export const BLUEPRINT_ART = "blueprint.png";

export const INGREDIENT_STATE_LABELS: Record<IngredientState, string> = {
  owned: "Owned",
  partial: "Partial",
  missing: "Missing",
  buildable: "Buildable",
  blocked: "Blocked by a missing ingredient",
  blueprint_missing: "Blueprint missing",
  master_first: "Master first",
  building: "Building in the Foundry",
};

/** The tooltip leaves these out, because the count already says it. */
export const STOCK_STATES: ReadonlySet<IngredientState> = new Set(["owned", "partial", "missing"]);

export const INGREDIENT_STATE_MARKS: Partial<Record<IngredientState, string>> = {
  master_first: "★",
  building: "◷",
};
