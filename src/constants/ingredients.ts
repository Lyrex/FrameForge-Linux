import type { IngredientState } from "../types/mastery";

/** The inventory cache's key for the credit balance. */
export const CREDITS_PATH = "/_currency/Credits";

/** The tooltip's third line. The stock states are left out because the count already says it. */
export const INGREDIENT_STATE_LABELS: Record<IngredientState, string | null> = {
  owned: null,
  partial: null,
  missing: null,
  buildable: "Buildable",
  blocked: "Blocked by a missing ingredient",
  master_first: "Master first",
  building: "Building in the Foundry",
};

export const INGREDIENT_STATE_MARKS: Partial<Record<IngredientState, string>> = {
  master_first: "★",
  building: "◷",
};
