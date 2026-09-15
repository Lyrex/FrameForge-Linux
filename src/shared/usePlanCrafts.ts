import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { TAURI_COMMANDS } from "../constants/tauri";
import type { CraftPlan } from "../types/mastery";

/** The `inventory` argument only triggers a re-plan, since the backend reads the inventory cache itself.
 *  Targets share one stock ledger in list order unless `standalone`, where each sees the whole stock. */
export function usePlanCrafts(uniqueNames: string[], inventory: unknown, standalone = false): Record<string, CraftPlan> {
  const [plans, setPlans] = useState<Record<string, CraftPlan>>({});
  useEffect(() => {
    if (uniqueNames.length === 0) { setPlans({}); return; }
    let stale = false;
    invoke<CraftPlan[]>(TAURI_COMMANDS.PLAN_CRAFTS, { uniqueNames, standalone })
      .then(result => { if (!stale) setPlans(Object.fromEntries(uniqueNames.map((id, i) => [id, result[i]]))); })
      .catch(() => {});
    return () => { stale = true; };
  }, [uniqueNames, inventory, standalone]);
  return plans;
}
