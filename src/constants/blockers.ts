import type { Blocker } from "../types/mastery";

export function blockerText(blocker: Blocker): string {
  switch (blocker.kind) {
    case "still_building": return "Still building";
    case "mastery_rank_below": return `Requires MR ${blocker.required}`;
    case "mastery_rank_unknown": return "Mastery Rank unknown";
    case "credits_short": return `Needs ${blocker.short.toLocaleString("en-US")} more credits`;
    case "credit_cost_unknown": return "Credit cost unknown";
    case "credits_unknown": return "Credits unknown";
    case "standing_unknown": return "Standing unknown";
    case "standing_rank_below": return `Rank ${blocker.required} with ${blocker.syndicate} needed`;
    case "standing_short": return `${blocker.short.toLocaleString("en-US")} standing short`;
    case "drop_sources_unknown": return "Drop sources unknown";
    case "missing_gate": return `${blocker.name} not cleared`;
    case "junction_tasks_unknown": return "Junction tasks unknown";
    case "node_unlock_unknown": return "Node unlock unknown";
    case "baro_away": return "Baro away";
    case "baro_not_stocking": return "Not in Baro's stock";
    case "baro_visit_unknown": return "Baro's schedule unknown";
  }
}
