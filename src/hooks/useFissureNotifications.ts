import { useEffect, useRef } from "react";
import { collectNewMatches } from "../fissureAlerts";
import { notify } from "../lib/notify";
import { fmtMs } from "../TimerHelper";
import { useWorldState } from "../worldstate";
import type { FissureWatch } from "../types/settings";
import type { SeenFissures } from "../types/worldstate";

export function useFissureNotifications(
  fissureWatches: FissureWatch[],
  fissureNotifications: boolean,
) {
  const restoredWatchIdsRef = useRef<Set<string>>(new Set());
  const knownWatchIdsRef = useRef<Set<string>>(new Set());
  const seenFissuresRef = useRef<SeenFissures>(new Map());
  const { worldState } = useWorldState(fissureNotifications && fissureWatches.length > 0);

  useEffect(() => {
    if (!fissureNotifications) {
      for (const watch of fissureWatches) restoredWatchIdsRef.current.add(watch.id);
      seenFissuresRef.current = new Map();
      return;
    }
    if (!worldState) return;

    for (const watch of fissureWatches) {
      if (!knownWatchIdsRef.current.has(watch.id)) {
        restoredWatchIdsRef.current.add(watch.id);
        knownWatchIdsRef.current.add(watch.id);
      }
    }

    const { fresh, live } = collectNewMatches(worldState, fissureWatches, seenFissuresRef.current, restoredWatchIdsRef.current);
    seenFissuresRef.current = live;
    if (fresh.length === 0) return;

    const suffix = (variant: string, separator: string) =>
      variant === "hard" ? `${separator}Steel Path` : variant === "storm" ? `${separator}Void Storm` : "";

    if (fresh.length === 1) {
      const { f, variant } = fresh[0];
      const remaining = new Date(f.expiry).getTime() - Date.now();
      void notify(
        `${f.tier} ${f.missionType}${suffix(variant, " · ")}`,
        remaining > 0 ? `${f.node} — ${fmtMs(remaining)} left` : f.node,
      );
      return;
    }

    const shown = fresh.slice(0, 5).map(({ f, variant }) =>
      `${f.tier} ${f.missionType}${suffix(variant, " ")} — ${f.node}`);
    if (fresh.length > shown.length) shown.push(`+${fresh.length - shown.length} more`);
    void notify(`${fresh.length} new fissures`, shown.join("\n"));
  }, [worldState, fissureWatches, fissureNotifications]);
}
