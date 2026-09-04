import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { fmtMs } from "./TimerHelper";
import { ensurePermission } from "./notify";
import { clampLead, MAX_LEAD_MINS, MIN_LEAD_MINS, type Schedule, type ScheduleEntry } from "./arbitrationAlerts";
import "./Arbitrations.css";

// The backend caches the feed for an hour, so this poll only costs IPC; it is
// what picks up the hourly refresh and rolls the window forward.
const POLL_MS = 60_000;

const fmtTime = (unix: number) =>
  new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const dayLabel = (unix: number) =>
  new Date(unix * 1000).toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" });

// Favorites and lead time are owned by App: the alerts have to keep firing
// while the user is looking at another module, and this component is unmounted
// for all of that time.
type Props = {
  favorites: string[];
  onToggleFavorite: (nodeId: string) => void;
  leadMins: number;
  onLeadChange: (mins: number) => void;
};

export default function Arbitrations({ favorites, onToggleFavorite, leadMins, onLeadChange }: Props) {
  const [schedule, setSchedule] = useState<Schedule | null>(null);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());
  const [permissionDenied, setPermissionDenied] = useState(false);
  const [leadDraft, setLeadDraft] = useState(String(leadMins));

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

  useEffect(() => setLeadDraft(String(leadMins)), [leadMins]);

  // Favorites can predate any permission prompt, and notify() will not raise
  // one itself. Opening this module is the gesture that earns the dialog.
  useEffect(() => {
    if (favorites.length > 0) void ensurePermission().then(granted => setPermissionDenied(!granted));
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Committed on blur rather than per keystroke: clamping while the field is
  // half-typed rewrites what the user is in the middle of entering.
  const commitLead = () => {
    const mins = clampLead(Number(leadDraft));
    onLeadChange(mins);
    setLeadDraft(String(mins));
  };

  const toggleFavorite = async (nodeId: string) => {
    const adding = !favorites.includes(nodeId);
    onToggleFavorite(nodeId);
    // Prompt from the star rather than from the alert timer, so the OS dialog
    // arrives while the user is thinking about arbitration alerts.
    if (adding) setPermissionDenied(!(await ensurePermission()));
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
      onClick={() => void toggleFavorite(e.node_id)}
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

      <div className="arb-alerts">
        <label>
          Alert me
          <input
            type="number"
            min={MIN_LEAD_MINS}
            max={MAX_LEAD_MINS}
            value={leadDraft}
            onChange={e => setLeadDraft(e.target.value)}
            onBlur={commitLead}
            onKeyDown={e => { if (e.key === "Enter") e.currentTarget.blur(); }}
          />
          minutes before a favorited node starts
        </label>
        {favorites.length === 0 && <span>Star a node to be alerted.</span>}
      </div>
      {permissionDenied && (
        <div className="arb-alerts-denied">FrameForge cannot send notifications. Check its permission in your system settings.</div>
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
