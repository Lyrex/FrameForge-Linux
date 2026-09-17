import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./Mastery.css";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import { PREFERENCE_KEYS } from "../constants/preferences";
import type { ClockFormat } from "../lib/clockFormat";
import { parseControls, purchaseSlugs, VIEW_OPTIONS, type MasteryControls, type ResultView } from "./suggestions";
import { pillSummary, progressText } from "./topBar";
import Collection from "./Collection";
import WhatNext from "./WhatNext";
import TargetMr from "./TargetMr";
import type { InventoryItem } from "../types/items";
import type { MasteryCounts, MasteryOverview, MasteryProvenance } from "../types/mastery";

export function Progress({ counts, label }: { counts: MasteryCounts; label: string }) {
  const { text, title } = progressText(counts, label);
  return (
    <div className="mst-progress-wrap" title={title}>
      <div className="mst-progress-bar">
        <div className="mst-progress-fill" style={{ width: counts.total > 0 ? `${(counts.mastered / counts.total) * 100}%` : "0%" }} />
      </div>
      <span className="mst-progress-label">{text}</span>
    </div>
  );
}

function ProvenancePill({ provenance, now, clockFormat }: { provenance: MasteryProvenance; now: number; clockFormat: ClockFormat }) {
  const { state, text, title } = pillSummary(provenance, now, clockFormat);
  return <span className={`mst-pill mst-pill-${state}`} title={title}>{text}</span>;
}

interface Props {
  /** Only a change signal: the overview itself comes from the backend. */
  inventory: Record<string, InventoryItem>;
  refreshKey: number;
  clockFormat: ClockFormat;
  tracked: string[];
  onTrackToggle: (uniqueName: string) => void;
}

export default function Mastery({ inventory, refreshKey, clockFormat, tracked, onTrackToggle }: Props) {
  const [overview, setOverview] = useState<MasteryOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [controls, setControls] = useState<MasteryControls>(() => parseControls(localStorage.getItem(PREFERENCE_KEYS.MASTERY_CONTROLS)));
  // A re-observation and the exclusion settings leave the inventory prop
  // untouched, so the backend announces them.
  const [observationKey, setObservationKey] = useState(0);
  const [planView, setPlanView] = useState<ResultView | null>(null);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  useEffect(() => {
    const refetch = () => setObservationKey(k => k + 1);
    // An unchanged scan moves only the observation stamps, which is not
    // worth rerunning the planning pass for.
    const restamp = ({ payload: provenance }: { payload: MasteryProvenance }) =>
      setOverview(o => o && { ...o, provenance });
    const unlisten = Promise.all([
      listen(TAURI_EVENTS.MASTERY_UPDATE, refetch),
      listen<MasteryProvenance>(TAURI_EVENTS.MASTERY_OBSERVED, restamp),
      listen(TAURI_EVENTS.SETTINGS_UPDATED, refetch),
    ]);
    const tick = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 60_000);
    return () => { clearInterval(tick); unlisten.then(fs => fs.forEach(f => f())); };
  }, []);

  useEffect(() => {
    let stale = false;
    invoke<MasteryOverview>(TAURI_COMMANDS.GET_MASTERY_OVERVIEW)
      .then(o => { if (!stale) { setOverview(o); setError(null); setNow(Math.floor(Date.now() / 1000)); } })
      .catch(e => { if (!stale) setError(String(e)); });
    return () => { stale = true; };
  }, [inventory, refreshKey, observationKey]);

  // Quotes are fetched only once the platinum view asks for them, a few a
  // second. Each answer changes the ranking, so the overview refetches, at
  // most once every couple of seconds while the answers stream in.
  const platinum = (controls.view === "whatnext" && controls.result === "platinum") || (controls.view === "target" && planView === "platinum");
  useEffect(() => {
    if (!platinum || !overview) return;
    const slugs = purchaseSlugs(overview.opportunities);
    if (slugs.length === 0) return;
    invoke(TAURI_COMMANDS.START_WFM_QUEUE)
      .then(() => invoke(TAURI_COMMANDS.WFM_QUEUE_PRICES, { urlNames: slugs }))
      .catch(() => {});
  }, [platinum, overview]);
  const refetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (!platinum) return;
    const unlisten = listen(TAURI_EVENTS.WFM_PRICE_UPDATE, () => {
      if (refetchTimer.current) return;
      refetchTimer.current = setTimeout(() => { refetchTimer.current = null; setObservationKey(k => k + 1); }, 2_000);
    });
    return () => {
      unlisten.then(f => f());
      if (refetchTimer.current) { clearTimeout(refetchTimer.current); refetchTimer.current = null; }
    };
  }, [platinum]);

  // No event fires when a build finishes, so refetch at the earliest completion time.
  useEffect(() => {
    const pending = (overview?.opportunities ?? [])
      .map(o => o.build_completion_ms).filter((ms): ms is number => ms != null && ms > Date.now());
    if (pending.length === 0) return;
    const t = setTimeout(() => setObservationKey(k => k + 1), Math.min(...pending) - Date.now());
    return () => clearTimeout(t);
  }, [overview]);

  useEffect(() => {
    localStorage.setItem(PREFERENCE_KEYS.MASTERY_CONTROLS, JSON.stringify(controls));
  }, [controls]);

  const update = (patch: Partial<MasteryControls>) => setControls(c => ({ ...c, ...patch }));

  return (
    <div className="mst-root">
      <div className="mst-toolbar mst-views">
        <div role="group" aria-label="Mastery view" className="mst-view-switch">
          {VIEW_OPTIONS.map(v => (
            <button
              key={v.key}
              aria-pressed={controls.view === v.key}
              className={`mst-filter-btn ${controls.view === v.key ? "active" : ""}`}
              onClick={() => update({ view: v.key })}
            >
              {v.label}
            </button>
          ))}
        </div>
        {overview && <Progress counts={overview.counts} label="All" />}
        {overview && <ProvenancePill provenance={overview.provenance} now={now} clockFormat={clockFormat} />}
        <label className="mst-select" title="Only what is not blocked, without the filters">
          <input type="checkbox" checked={controls.easy} onChange={e => update({ easy: e.target.checked })} />
          Easy mode
        </label>
      </div>

      {error && <div className="mst-body"><div className="mst-empty">Mastery overview unavailable: {error}</div></div>}
      {!error && !overview && <div className="mst-body"><div className="mst-empty">Loading mastery…</div></div>}
      {overview && controls.view === "collection" && <Collection overview={overview} />}
      {overview && controls.view === "whatnext" && (
        <WhatNext overview={overview} controls={controls} onChange={update} nowMs={now * 1000} clockFormat={clockFormat} />
      )}
      {overview && controls.view === "target" && (
        <TargetMr overview={overview} controls={controls} onChange={update} nowMs={now * 1000} clockFormat={clockFormat}
          tracked={tracked} onTrackToggle={onTrackToggle} onView={setPlanView} />
      )}
    </div>
  );
}
