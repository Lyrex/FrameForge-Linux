import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { fmtMs } from "./TimerHelper";
import {
  completed, filterRuns, summarize, MISSION_TYPES,
  type Breakdown, type Filters, type MissionType, type RunRecord,
} from "./arbitrationAnalytics";
import Sparkline from "./Sparkline";
import { TierBadge } from "./TierSelect";
import { fmtClock, type ClockFormat } from "./clockFormat";
import "./Report.css";

const RANGES: { label: string; value: number | "all" }[] = [
  { label: "7d", value: 7 }, { label: "30d", value: 30 }, { label: "90d", value: 90 }, { label: "All", value: "all" },
];

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const fmtRate = (r: number | null) => (r === null ? "—" : r.toFixed(2));
const fmtDate = (iso: string | null, format: ClockFormat, locale: string) => {
  if (iso === null) return "unknown time";
  const when = new Date(iso);
  // The month name stays English; only the time follows the hour format.
  const date = when.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  return `${date}, ${fmtClock(when.getTime() / 1000, format, locale)}`;
};
const endLabel: Record<Exclude<RunRecord["end_reason"], "mission_end">, string> = {
  aborted: "aborted", new_mission: "left early", unterminated: "unfinished",
};

function BreakdownTable({ label, rows, name }: { label: string; rows: Breakdown[]; name: (key: string) => string }) {
  if (rows.length === 0) return null;
  return (
    <div className="arb-breakdown">
      <div className="report-stat-label">{label}</div>
      <div className="arb-table">
        <div className="arb-table-head"><span>{label}</span><span>Runs</span><span>Vitus</span><span>Vitus/min</span></div>
        {rows.map(r => (
          <div className="arb-table-row" key={r.key}>
            <span className="arb-table-name">{name(r.key)}</span>
            <span className="arb-table-num">{r.runs}{r.completed < r.runs && <span className="arb-muted"> ({r.runs - r.completed} incomplete)</span>}</span>
            <span className="arb-table-num">{Math.round(r.vitus)}</span>
            <span className="arb-table-num">{fmtRate(r.perMinute)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Run history tab ────────────────────────────────────────────────────────────

export default function ArbitrationHistory({ clockFormat, systemLocale }: { clockFormat: ClockFormat; systemLocale: string }) {
  const [runs, setRuns] = useState<RunRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filters, setFilters] = useState<Filters>({ days: 30, missionType: "all" });
  const [confirmUid, setConfirmUid] = useState<string | null>(null);

  // The date window is measured from now, so the tab has to notice time
  // passing or a run keeps sitting inside "7d" for as long as it stays open.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(tick);
  }, []);

  const load = useCallback(() =>
    invoke<RunRecord[]>("get_arbitration_runs")
      .then(r => { setRuns(r); setError(null); })
      .catch(e => setError(String(e))), []);

  useEffect(() => {
    void load();
    // Runs land from the log watcher while this tab is open.
    const unlisten = listen("arbitration-runs-changed", () => void load());
    return () => { void unlisten.then(fn => fn()); };
  }, [load]);

  const shown = useMemo(() => (runs ? filterRuns(runs, filters, now) : []), [runs, filters, now]);
  const summary = useMemo(() => summarize(shown), [shown]);

  const setFilter = (patch: Partial<Filters>) => {
    setConfirmUid(null);
    setFilters(f => ({ ...f, ...patch }));
  };

  // The list is reloaded rather than edited in place: the backend's answer
  // is the only thing that says whether the run is gone.
  const remove = async (uid: string) => {
    setConfirmUid(null);
    try {
      await invoke("delete_arbitration_run", { uid });
    } catch (e) {
      setError(String(e));
    }
    await load();
  };

  if (error && !runs) return <div className="timer-error">{error}</div>;
  if (!runs) return <div className="timer-loading">Loading run history…</div>;
  if (runs.length === 0) {
    return (
      <div className="report-empty">
        {error && <div className="timer-error">{error}</div>}
        <div className="report-empty-icon">🏛️</div>
        <div className="report-empty-title">No arbitration runs recorded yet</div>
        <div className="report-empty-desc">
          Runs are recorded from the game's log while FrameForge is open,
          <br />
          and runs from the current log are picked up on startup.
        </div>
      </div>
    );
  }

  return (
    <div className="arb-history">
      {error && <div className="timer-error">{error}</div>}

      <div className="arb-filters">
        <div className="report-tf-btns">
          {RANGES.map(r => (
            <button key={r.label} className={`report-tf-btn${filters.days === r.value ? " active" : ""}`}
              onClick={() => setFilter({ days: r.value })}>{r.label}</button>
          ))}
        </div>
        <div className="report-tf-btns">
          {(["all", ...MISSION_TYPES] as (MissionType | "all")[]).map(t => (
            <button key={t} className={`report-tf-btn${filters.missionType === t ? " active" : ""}`}
              onClick={() => setFilter({ missionType: t })}>{cap(t)}</button>
          ))}
        </div>
        <span className="arb-muted arb-filter-count">{shown.length} of {runs.length} runs</span>
      </div>

      <div className="report-card">
        <div className="report-stat-label">Vitus per minute, run by run</div>
        <Sparkline values={summary.rate} empty="No completed runs in this range" />
        <div className="report-card-stats">
          <div className="report-stat"><span className="report-stat-label">Runs</span><span className="report-stat-value">{summary.runs}</span></div>
          <div className="report-stat"><span className="report-stat-label">Incomplete</span><span className="report-stat-value">{summary.incomplete}</span></div>
          <div className="report-stat"><span className="report-stat-label">Playtime (all runs)</span><span className="report-stat-value">{fmtMs(summary.playtimeSec * 1000)}</span></div>
          <div className="report-stat"><span className="report-stat-label">Kills</span><span className="report-stat-value">{summary.kills.toLocaleString()}</span></div>
          <div className="report-stat"><span className="report-stat-label">Vitus (est.)</span><span className="report-stat-value">{Math.round(summary.vitus)}</span></div>
          <div className="report-stat"><span className="report-stat-label">Vitus/min</span><span className="report-stat-value">{fmtRate(summary.perMinute)}</span></div>
        </div>
      </div>

      <BreakdownTable label="Node" rows={summary.byNode} name={k => k} />
      <BreakdownTable label="Mission type" rows={summary.byMissionType} name={cap} />

      <div className="report-stat-label">Runs</div>
      <div className="arb-runs">
        {shown.map(run => (
          <div className={`arb-run${completed(run) ? "" : " arb-run-incomplete"}`} key={run.uid}>
            <span className="arb-run-when">{fmtDate(run.started_at, clockFormat, systemLocale)}</span>
            <span className="arb-run-main">
              <span className="timer-name"><TierBadge tier={run.tier} />{run.node}</span>
              <span className="arb-muted">
                {cap(run.mission_type)}
                {!completed(run) && <span className="arb-run-flag"> · {endLabel[run.end_reason as keyof typeof endLabel]}</span>}
              </span>
            </span>
            <span className="arb-run-num">{fmtMs(run.duration_sec * 1000)}</span>
            <span className="arb-run-num">{run.waves > 0 ? `${run.waves} waves` : `${run.rotations} rotations`}</span>
            <span className="arb-run-num">{run.kills} kills</span>
            <span className="arb-run-num">{fmtRate(run.vitus_per_minute)}/min</span>
            {confirmUid === run.uid ? (
              <span className="report-confirm-row">
                <span className="report-confirm-msg">Delete run?</span>
                <button className="report-confirm-yes" onClick={() => void remove(run.uid)}>Delete</button>
                <button className="report-confirm-no" onClick={() => setConfirmUid(null)}>Cancel</button>
              </span>
            ) : (
              <button className="report-remove-btn" title="Delete run" onClick={() => setConfirmUid(run.uid)}>×</button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
