import { useEffect, useRef, useState } from "react";
import { fmtMs } from "./TimerHelper";
import { ensurePermission, permissionGranted } from "./notify";
import { clampLead, MAX_LEAD_MINS, MIN_LEAD_MINS, type ScheduleEntry } from "./arbitrationAlerts";
import { useArbitrationSchedule } from "./arbitrationSchedule";
import ArbitrationHistory from "./ArbitrationHistory";
import "./Arbitrations.css";
import "./Report.css";

const fmtTime = (unix: number) =>
  new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const dayLabel = (unix: number) =>
  new Date(unix * 1000).toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" });

// Favorites and lead time are owned by App: the alerts have to keep firing
// while the user is looking at another module, and this component is unmounted
// for all of that time. Permission state too: a denial only this component
// knows about is invisible for as long as the user is elsewhere.
type Props = {
  favorites: string[];
  onToggleFavorite: (nodeId: string) => void;
  leadMins: number;
  onLeadChange: (mins: number) => void;
  permissionDenied: boolean;
  onPermissionChange: (denied: boolean) => void;
};

export default function Arbitrations(props: Props) {
  const [tab, setTab] = useState<"schedule" | "history">("schedule");
  return (
    <div className="arb">
      <div className="sub-tabs">
        <button className={tab === "schedule" ? "active" : ""} onClick={() => setTab("schedule")}>Schedule</button>
        <button className={tab === "history" ? "active" : ""} onClick={() => setTab("history")}>Run history</button>
      </div>
      {tab === "schedule" ? <Schedule {...props} /> : <ArbitrationHistory />}
    </div>
  );
}

function Schedule(
  { favorites, onToggleFavorite, leadMins, onLeadChange, permissionDenied, onPermissionChange }: Props,
) {
  const { schedule, error, refresh: fetchSchedule } = useArbitrationSchedule(true);
  const [now, setNow] = useState(() => Date.now());
  const [leadDraft, setLeadDraft] = useState(String(leadMins));

  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, []);

  useEffect(() => setLeadDraft(String(leadMins)), [leadMins]);

  // Favorites arrive from settings after mount, so this waits for them rather
  // than reading the empty array the first render sees.
  const askedRef = useRef(false);
  useEffect(() => {
    if (favorites.length === 0 || askedRef.current) return;
    askedRef.current = true;
    void ensurePermission().then(granted => onPermissionChange(!granted));
  }, [favorites.length]);

  // Permission is granted in system settings, so the app hears about it by
  // getting the window back, not by anything happening inside it. Read-only:
  // a dialog raised by a focus event is one the user did nothing to invite.
  useEffect(() => {
    if (favorites.length === 0) return;
    const recheck = () => void permissionGranted().then(granted => onPermissionChange(!granted));
    window.addEventListener("focus", recheck);
    return () => window.removeEventListener("focus", recheck);
  }, [favorites.length, onPermissionChange]);

  // Committed on blur rather than per keystroke: clamping while the field is
  // half-typed rewrites what the user is in the middle of entering.
  const commitLead = () => {
    // A cleared field is someone retyping, not a request for the shortest lead
    // the app allows; Number("") would otherwise commit it as zero.
    if (leadDraft.trim() === "") {
      setLeadDraft(String(leadMins));
      return;
    }
    const mins = clampLead(Number(leadDraft));
    onLeadChange(mins);
    setLeadDraft(String(mins));
  };

  const toggleFavorite = async (nodeId: string) => {
    const adding = !favorites.includes(nodeId);
    onToggleFavorite(nodeId);
    if (adding) onPermissionChange(!(await ensurePermission()));
  };

  if (!schedule) {
    return error
      ? <div className="arb-scroll"><div className="timer-error">{error} <button onClick={fetchSchedule}>Retry</button></div></div>
      : <div className="arb-scroll"><div className="timer-loading">Loading arbitration schedule…</div></div>;
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
    <div className="arb-scroll">
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
