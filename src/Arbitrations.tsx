import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fmtMs } from "./TimerHelper";
import "./Arbitrations.css";

export type ScheduleEntry = {
  start: number;
  end: number;
  node_id: string;
  node: string;
  region: string;
  mission_type: string;
  faction: string;
};

type Schedule = {
  entries: ScheduleEntry[];
  source: "fresh" | "refreshed" | "refreshing" | "stale" | "fallback";
  warning: string | null;
};

// The backend caches the feed for an hour, so this poll only costs IPC; it is
// what picks up the hourly refresh and rolls the window forward.
const POLL_MS = 60_000;

// Favorites live under their own settings key. save_settings merges keys, so
// this never touches, and is never clobbered by, the main window's own save.
const FAVORITES_KEY = "arbitrationFavorites";

const fmtTime = (unix: number) =>
  new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const dayLabel = (unix: number) =>
  new Date(unix * 1000).toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" });

export default function Arbitrations() {
  const [schedule, setSchedule] = useState<Schedule | null>(null);
  const [error, setError] = useState("");
  const [favorites, setFavorites] = useState<string[]>([]);
  const [now, setNow] = useState(() => Date.now());

  const fetchSchedule = useCallback(() => {
    invoke<Schedule>("fetch_arbitration_schedule")
      .then(s => { setSchedule(s); setError(""); })
      .catch(e => setError(String(e)));
  }, []);

  useEffect(() => {
    fetchSchedule();
    const poll = setInterval(fetchSchedule, POLL_MS);
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => { clearInterval(poll); clearInterval(tick); };
  }, [fetchSchedule]);

  useEffect(() => {
    invoke<string>("load_settings").then(raw => {
      try {
        const saved = raw.trim() ? JSON.parse(raw)[FAVORITES_KEY] : [];
        if (Array.isArray(saved)) setFavorites(saved.filter(x => typeof x === "string"));
      } catch (e) {
        console.error("arbitration favorites unreadable", e);
      }
    }).catch(e => console.error("load_settings failed", e));
  }, []);

  const toggleFavorite = (nodeId: string) => {
    const next = favorites.includes(nodeId) ? favorites.filter(x => x !== nodeId) : [...favorites, nodeId];
    setFavorites(next);
    invoke("save_settings", { json: JSON.stringify({ [FAVORITES_KEY]: next }) })
      .catch(e => console.error("saving arbitration favorites failed", e));
  };

  if (!schedule) {
    return error
      ? <div className="arb"><div className="timer-error">{error} <button onClick={fetchSchedule}>Retry</button></div></div>
      : <div className="arb"><div className="timer-loading">Loading arbitration schedule…</div></div>;
  }

  const nowSec = now / 1000;
  const live = schedule.entries.filter(e => e.end > nowSec);
  const current = live.find(e => e.start <= nowSec);
  const upcoming = live.filter(e => e.start > nowSec);
  const stale = schedule.source === "stale" || error;
  const isFav = (e: ScheduleEntry) => favorites.includes(e.node_id);

  const star = (e: ScheduleEntry) => (
    <button
      className={`timer-star ${isFav(e) ? "fav" : ""}`}
      onClick={() => toggleFavorite(e.node_id)}
      title={isFav(e) ? "Unfavorite node" : "Favorite node"}
    >★</button>
  );
  const detail = (e: ScheduleEntry) =>
    [e.mission_type, e.faction].filter(Boolean).join(" · ");

  let lastDay = "";
  return (
    <div className="arb">
      {stale && (
        <div className="arb-stale" title={schedule.warning ?? error}>
          Showing the last schedule that loaded; refreshing failed.
          <button onClick={fetchSchedule}>Retry</button>
        </div>
      )}

      <div className="timer-group-label">Now</div>
      {current ? (
        <div className={`arb-current ${isFav(current) ? "arb-fav" : ""}`}>
          {star(current)}
          <div className="arb-current-body">
            <div className="arb-current-node">
              {current.node}{current.region && <span className="arb-region"> ({current.region})</span>}
            </div>
            <div className="arb-detail">{detail(current)}</div>
          </div>
          <div className="arb-current-cd">
            <div className="timer-cd">{fmtMs(current.end * 1000 - now)}</div>
            <div className="timer-until">remaining</div>
          </div>
        </div>
      ) : (
        <div className="timer-empty">No arbitration in the schedule for this hour.</div>
      )}

      {upcoming.length === 0 && <div className="timer-empty">No upcoming arbitrations in the feed.</div>}
      {upcoming.map(e => {
        const day = dayLabel(e.start);
        const header = day !== lastDay ? <div className="timer-group-label">{day}</div> : null;
        lastDay = day;
        return (
          <div key={e.start}>
            {header}
            <div className={`timer-row ${isFav(e) ? "arb-fav" : ""}`}>
              {star(e)}
              <span className="arb-time">{fmtTime(e.start)}</span>
              <span className="timer-name">
                {e.node}{e.region && <span className="arb-region"> ({e.region})</span>}
              </span>
              <span className="arb-detail">{detail(e)}</span>
              <span className="timer-until">in {fmtMs(e.start * 1000 - now)}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}
