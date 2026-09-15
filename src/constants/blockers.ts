import type { Blocker } from "../types/mastery";

export function blockerText(blocker: Blocker): string {
  switch (blocker.kind) {
    case "still_building": return "Still building";
    case "mastery_rank_below": return `Requires MR ${blocker.required}`;
    case "mastery_rank_not_observed": return "Mastery Rank not observed";
    case "credits_short": return `Needs ${blocker.short.toLocaleString("en-US")} more credits`;
    case "credit_cost_unknown": return "Credit cost unknown";
    case "credits_not_observed": return "Credits not observed";
    case "standing_not_observed": return "Standing not observed";
    case "drop_sources_unknown": return "Drop sources unknown";
    case "missing_gate": return `${blocker.name} not cleared`;
    case "junction_tasks_not_observed": return "Junction tasks not observed";
    case "node_unlock_not_observed": return "Node unlock not observed";
  }
}
