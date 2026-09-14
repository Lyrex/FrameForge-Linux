import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ItemImg from "../ItemImg";
import Filters from "./Filters";
import { OpportunityRow } from "./WhatNext";
import { TAURI_COMMANDS } from "../constants/tauri";
import type { ClockFormat } from "../lib/clockFormat";
import { addable, candidates, moved, planView, prefill, rangeText, summaryText, TARGET_RANK_MAX } from "./plan";
import { RESULT_OPTIONS, remainingText, type MasteryControls, type ResultView } from "./suggestions";
import type { MasteryOverview, MasteryPlan, PlanEntry, PlanEvaluation } from "../types/mastery";

// Sixty rows are enough to browse, and the search narrows the rest.
const ADD_LIST_LIMIT = 60;

interface Props {
  overview: MasteryOverview;
  controls: MasteryControls;
  onChange: (patch: Partial<MasteryControls>) => void;
  nowMs: number;
  clockFormat: ClockFormat;
  /** A switch reloads the plan, since each player has their own. */
  playerName: string | null;
  tracked: string[];
  onTrackToggle: (uniqueName: string) => void;
  /** Tells the parent which result view the plan draws from, so a platinum plan gets quotes. */
  onView: (view: ResultView) => void;
}

function Placeholder({ path, entry, overview }: { path: string; entry: PlanEntry | null; overview: MasteryOverview }) {
  const source = entry?.source ?? overview.categories.flatMap(c => c.sources).find(s => s.unique_name === path) ?? null;
  const name = source?.name ?? path.split("/").pop() ?? path;
  return (
    <div className={`mst-opp ${entry?.completed ? "mst-plan-done" : ""}`}>
      <ItemImg imageName={source?.image_name ?? undefined} fallback={<div className="img-fallback">{name[0]?.toUpperCase() ?? "?"}</div>} />
      <div className="mst-opp-main">
        <div className="mst-opp-title">
          <span className="mst-name">{name}</span>
          {source && <span className="mst-mr">{source.category}</span>}
        </div>
        {entry && entry.notes.length > 0 && <div className="mst-opp-blockers">{entry.notes.join(" · ")}</div>}
        {!entry && <div className="mst-opp-detail">Evaluating…</div>}
      </div>
      {entry?.completed && <span className="mst-pill mst-pill-done">Completed</span>}
      {source && <span className={`mst-rank rank-${source.state}`}>{remainingText(source.remaining_mastery)}</span>}
    </div>
  );
}

export default function TargetMr({ overview, controls, onChange, nowMs, clockFormat, playerName, tracked, onTrackToggle, onView }: Props) {
  const [plan, setPlan] = useState<MasteryPlan | null>(null);
  // A plan that was never saved prefills itself once the gap is known. A cleared plan stays empty.
  const [fresh, setFresh] = useState(false);
  const [evaluation, setEvaluation] = useState<PlanEvaluation | null>(null);
  const [search, setSearch] = useState("");
  const [showRange, setShowRange] = useState(false);

  useEffect(() => {
    let stale = false;
    setPlan(null);
    setEvaluation(null);
    invoke<MasteryPlan | null>(TAURI_COMMANDS.LOAD_MASTERY_PLAN)
      .then(saved => {
        if (stale) return;
        if (saved) { setPlan({ ...saved, view: planView(saved.view) }); setFresh(false); }
        else { setPlan({ target: (overview.mastery_rank ?? 0) + 1, view: "suggestions", selections: [], allowances: {} }); setFresh(true); }
      })
      .catch(() => {});
    return () => { stale = true; };
  }, [playerName, overview.mastery_rank]); // eslint-disable-line

  // Every observation re-evaluates the same selections. Only the player changes them.
  useEffect(() => {
    if (!plan) return;
    let stale = false;
    invoke<PlanEvaluation>(TAURI_COMMANDS.EVALUATE_MASTERY_PLAN, { plan })
      .then(e => { if (!stale) setEvaluation(e); })
      .catch(() => {});
    return () => { stale = true; };
  }, [plan, overview]);

  const view = plan ? planView(plan.view) : "suggestions";
  useEffect(() => { onView(view); }, [view]); // eslint-disable-line
  const pool = useMemo(() => candidates(overview.opportunities, controls, view), [overview.opportunities, controls, view]);

  const save = (next: MasteryPlan) => {
    setPlan(next);
    setFresh(false);
    invoke(TAURI_COMMANDS.SAVE_MASTERY_PLAN, { plan: next }).catch(() => {});
  };

  // Before the rank is observed the default target is MR 1 and the gap zero, so the prefill waits.
  // The load above runs again once the rank lands and picks the target from it.
  useEffect(() => {
    if (!fresh || !plan || overview.mastery_rank == null || evaluation?.gap == null) return;
    save({ ...plan, selections: prefill(pool, evaluation.gap) });
  }, [fresh, evaluation]); // eslint-disable-line

  if (!plan) return <div className="mst-body"><div className="mst-empty">Loading plan…</div></div>;

  const entries = evaluation && evaluation.entries.length === plan.selections.length ? evaluation.entries : null;
  const entryAt = (i: number): PlanEntry | null => entries && entries[i].unique_name === plan.selections[i] ? entries[i] : null;
  const toAdd = addable(pool, plan, search);
  const setTarget = (raw: string) => {
    const target = Math.min(TARGET_RANK_MAX, Math.max(1, Number.parseInt(raw, 10) || 1));
    if (target !== plan.target) save({ ...plan, target });
  };
  const setAllowance = (path: string, raw: string, max: number) => {
    const allowances = { ...plan.allowances };
    const count = Math.min(max, Math.max(0, Number.parseInt(raw, 10) || 0));
    if (count > 0) allowances[path] = count; else delete allowances[path];
    save({ ...plan, allowances });
  };

  return (
    <>
      <div className="mst-toolbar mst-plan-bar">
        <label className="mst-select">Target
          <input type="number" className="mst-plan-target" min={1} max={TARGET_RANK_MAX} value={plan.target} onChange={e => setTarget(e.target.value)} aria-label="Target Mastery Rank" />
        </label>
        <div role="group" aria-label="Prefill from" className="mst-view-switch">
          {RESULT_OPTIONS.map(v => (
            <button
              key={v.key}
              aria-pressed={view === v.key}
              className={`mst-filter-btn ${view === v.key ? "active" : ""}`}
              onClick={() => save({ ...plan, view: v.key })}
            >
              {v.label}
            </button>
          ))}
        </div>
        <button className="mst-filter-btn" disabled={evaluation?.gap == null} title="Fill the plan again from the chosen view, replacing the current selections"
          onClick={() => evaluation?.gap != null && save({ ...plan, selections: prefill(pool, evaluation.gap) })}>
          Regenerate
        </button>
        <button className="mst-filter-btn" disabled={plan.selections.length === 0} onClick={() => save({ ...plan, selections: [] })}>Clear all</button>
      </div>

      <div className="mst-toolbar mst-plan-summary">
        {evaluation ? (
          <>
            <span className="mst-plan-summary-text" title='Projection means "if these actions finish"'>{summaryText(evaluation, plan.target)}</span>
            {evaluation.total && (
              <button className="mst-filter-btn" aria-expanded={showRange} onClick={() => setShowRange(s => !s)}>{showRange ? "Hide range" : "Range"}</button>
            )}
            {showRange && <span className="mst-plan-range">{rangeText(evaluation)}</span>}
            {evaluation.rejected_allowances.length > 0 && (
              <span className="mst-opp-blockers">Allowance refused for unmastered or unowned equipment</span>
            )}
          </>
        ) : <span className="mst-plan-summary-text">Evaluating plan…</span>}
      </div>

      <Filters overview={overview} controls={controls} onChange={onChange} result={view} search={search} onSearch={setSearch} />

      <div className="mst-body">
        <section className="mst-group" aria-label="Plan">
          <div className="mst-group-header">Plan <span className="mst-count">{plan.selections.length}</span></div>
          {plan.selections.length === 0 && (
            <div className="mst-empty">
              {evaluation?.gap == null ? "Mastery Rank not observed yet. Add actions below, or regenerate once a scan has seen the account."
                : "Nothing planned. Regenerate to fill from the chosen view, or add actions below."}
            </div>
          )}
          <div className="mst-opp-list">
            {plan.selections.map((path, i) => {
              const entry = entryAt(i);
              const o = entry?.opportunity ?? null;
              const controlsFor = (
                <span className="mst-plan-controls">
                  {o?.craft && (
                    <button className={`mst-plan-btn mst-plan-star ${tracked.includes(path) ? "tracked" : ""}`} title={tracked.includes(path) ? "Tracked in Foundry" : "Track in Foundry"} onClick={() => onTrackToggle(path)}>
                      {tracked.includes(path) ? "★" : "☆"}
                    </button>
                  )}
                  <button className="mst-plan-btn" aria-label="Move up" disabled={i === 0} onClick={() => save({ ...plan, selections: moved(plan.selections, i, -1) })}>↑</button>
                  <button className="mst-plan-btn" aria-label="Move down" disabled={i === plan.selections.length - 1} onClick={() => save({ ...plan, selections: moved(plan.selections, i, 1) })}>↓</button>
                  <button className="mst-plan-btn" aria-label="Remove" onClick={() => save({ ...plan, selections: plan.selections.filter((_, j) => j !== i) })}>✕</button>
                </span>
              );
              return (
                <div key={`${path}#${i}`} className="mst-plan-row">
                  <span className="mst-plan-index">{i + 1}</span>
                  {o ? (
                    <OpportunityRow opportunity={{ ...o, blockers: [...o.blockers, ...(entry?.notes ?? [])] }} nowMs={nowMs} clockFormat={clockFormat}>
                      {controlsFor}
                    </OpportunityRow>
                  ) : (
                    <><Placeholder path={path} entry={entry} overview={overview} />{controlsFor}</>
                  )}
                  {entry && entry.allowable.length > 0 && (
                    <div className="mst-plan-allowances">
                      {entry.allowable.map(a => (
                        <label key={a.unique_name} className="mst-select" title="How many copies of this mastered piece the plan may spend as an ingredient">
                          Spend owned {a.name}
                          <input type="number" className="mst-plan-target" min={0} max={a.owned} value={plan.allowances[a.unique_name] ?? 0}
                            onChange={e => setAllowance(a.unique_name, e.target.value, a.owned)} />
                          <span className="mst-count">of {a.owned}</span>
                        </label>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </section>

        <section className="mst-group" aria-label="Add to plan">
          <div className="mst-group-header">
            Add from {RESULT_OPTIONS.find(v => v.key === view)?.label} <span className="mst-count">{toAdd.length}</span>
          </div>
          {toAdd.length === 0 && <div className="mst-empty">Nothing left to add from this view.</div>}
          <div className="mst-opp-list">
            {toAdd.slice(0, ADD_LIST_LIMIT).map(o => (
              <OpportunityRow key={o.unique_name} opportunity={o} nowMs={nowMs} clockFormat={clockFormat}>
                <span className="mst-plan-controls">
                  <button className="mst-plan-btn" aria-label={`Add ${o.name}`} onClick={() => save({ ...plan, selections: [...plan.selections, o.unique_name] })}>+</button>
                </span>
              </OpportunityRow>
            ))}
          </div>
          {toAdd.length > ADD_LIST_LIMIT && <div className="mst-empty">{toAdd.length - ADD_LIST_LIMIT} more; narrow the search.</div>}
        </section>
      </div>
    </>
  );
}
