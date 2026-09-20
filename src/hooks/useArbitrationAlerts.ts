import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fmtMs } from "../TimerHelper";
import { notify } from "../lib/notify";
import { runAlertPass, DEFAULT_LEAD_MINS, EVAL_INTERVAL_MS, type AlertRule, type ScheduleEntry } from "../arbitration/arbitrationAlerts";
import { useArbitrationSchedule } from "../arbitration/arbitrationSchedule";
import type { TierKey } from "../arbitration/arbitrationTiers";

/** The alert loop lives here rather than in Arbitrations because that module
 *  is not mounted until the user first opens it. */
export function useArbitrationAlerts(
  favorites: string[],
  alertTiers: TierKey[],
  leadMins: number,
  firedRef: React.MutableRefObject<string[]>,
) {
  const alertsOn = favorites.length > 0 || alertTiers.length > 0;
  const { schedule, error } = useArbitrationSchedule(alertsOn);

  // The loop reads its inputs from here rather than from the effect closure, so
  // starring a node changes what the next tick sees without tearing the timer
  // down and starting a fresh pass on top of one already running. This effect
  // has to stay above the loop's own, which reads the ref on its first tick.
  const inputsRef = useRef({ entries: [] as ScheduleEntry[], rule: {} as AlertRule, leadMins: DEFAULT_LEAD_MINS });
  useEffect(() => {
    inputsRef.current = {
      entries: schedule?.entries ?? [],
      rule: { favorites, tiers: alertTiers },
      leadMins,
    };
  });

  // A pass outlives its tick whenever the notification IPC is slow, and two
  // passes reading the same fired state would raise one occurrence twice.
  const checkingRef = useRef(false);

  useEffect(() => {
    if (alertsOn && error) {
      console.error("arbitration schedule unavailable, alerts paused:", error);
    }
  }, [alertsOn, error]);

  useEffect(() => {
    const check = async () => {
      // A prune raises nothing, so it need not wait for a pass already running;
      // queuing it behind the guard would drop it, since unstarring the last
      // node also stops the timer that would otherwise come back to it.
      if (alertsOn && checkingRef.current) return;
      checkingRef.current = true;
      try {
        const { entries, rule, leadMins } = inputsRef.current;
        const nowMs = Date.now();
        const fired = await runAlertPass(
          entries, rule, leadMins, firedRef.current, nowMs / 1000,
          e => notify(
            `Arbitration — ${e.node}${e.region ? ` (${e.region})` : ""}`,
            `${[e.mission_type, e.faction].filter(Boolean).join(" · ")} — ${e.start * 1000 > nowMs
              ? `starts in ${fmtMs(e.start * 1000 - nowMs)}`
              : `under way, ${fmtMs(e.end * 1000 - nowMs)} left`}`,
          ));
        if (fired === null) return;
        firedRef.current = fired;
        invoke("save_settings", { json: JSON.stringify({ arbitrationAlertsFired: fired }) })
          .catch(e => console.error("saving arbitration alert state failed", e));
      } finally {
        checkingRef.current = false;
      }
    };

    // Unstarring the last node still leaves keys behind, so one pass runs to
    // prune them; only a user with favorites keeps the timer.
    void check();
    if (!alertsOn) return;
    const poll = setInterval(check, EVAL_INTERVAL_MS);
    return () => clearInterval(poll);
  }, [alertsOn]); // eslint-disable-line
}
