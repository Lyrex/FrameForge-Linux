import { useMemo, useState } from "react";
import ItemImg from "../ItemImg";
import SearchBar from "../shared/SearchBar";
import { fmtClock, type ClockFormat } from "../lib/clockFormat";
import {
  actionText, detailText, remainingText, visibleOpportunities,
  AVAILABILITY_OPTIONS, DEFAULT_CONTROLS, PROGRESS_OPTIONS, RESULT_OPTIONS, SORT_OPTIONS, STAGE_LABELS, STAGE_ORDER,
  type AvailabilityFilter, type MasteryControls, type ProgressFilter, type Sort,
} from "./suggestions";
import type { MasteryOverview, Opportunity } from "../types/mastery";

const ACCESS_LABELS = { available: "Available", blocked: "Blocked", unknown: "Unknown access" } as const;

function OpportunityRow({ opportunity, nowMs, clockFormat }: { opportunity: Opportunity; nowMs: number; clockFormat: ClockFormat }) {
  const { access, blockers, build_completion_ms, category, image_name, mastery_req, name, remaining_mastery, state } = opportunity;
  const readyAt = build_completion_ms == null ? undefined : fmtClock(Math.floor(build_completion_ms / 1000), clockFormat);
  return (
    <div className={`mst-opp mst-opp-${access}`}>
      <ItemImg imageName={image_name ?? undefined} fallback={<div className="img-fallback">{name[0]?.toUpperCase() ?? "?"}</div>} />
      <div className="mst-opp-main">
        <div className="mst-opp-title">
          <span className="mst-name">{name}</span>
          <span className="mst-mr">{category}</span>
          {mastery_req != null && mastery_req > 0 && (
            <span className="mst-mr" title={`Mastery Rank ${mastery_req} required`}>MR{mastery_req}</span>
          )}
        </div>
        <div className="mst-opp-detail">{detailText(opportunity, nowMs, readyAt)}</div>
        {blockers.length > 0 && <div className="mst-opp-blockers">{blockers.join(" · ")}</div>}
      </div>
      <span className="mst-opp-action">{actionText(opportunity)}</span>
      <span className={`mst-pill mst-pill-${access}`}>{ACCESS_LABELS[access]}</span>
      <span className={`mst-rank rank-${state}`} title={remaining_mastery == null ? "Remaining mastery unknown: no account observation yet" : "Remaining mastery"}>
        {remainingText(remaining_mastery)}
      </span>
    </div>
  );
}

interface Props {
  overview: MasteryOverview;
  controls: MasteryControls;
  onChange: (patch: Partial<MasteryControls>) => void;
  nowMs: number;
  clockFormat: ClockFormat;
}

export default function WhatNext({ overview, controls, onChange, nowMs, clockFormat }: Props) {
  const [search, setSearch] = useState("");
  const category = overview.categories.some(c => c.category === controls.category) ? controls.category : null;
  const visible = useMemo(
    () => visibleOpportunities(overview.opportunities, { ...controls, category }, search),
    [overview.opportunities, controls, category, search]);
  const groups = STAGE_ORDER
    .map(stage => ({ stage, items: visible.filter(o => o.stage === stage) }))
    .filter(g => g.items.length > 0);
  const isFiltered = search !== "" || category != null || controls.progress !== "all" || controls.availability !== "all";

  return (
    <>
      <div className="mst-tabs" role="group" aria-label="Result view">
        {RESULT_OPTIONS.map(v => (
          <button
            key={v.key}
            aria-pressed={controls.result === v.key}
            className={`mst-tab ${controls.result === v.key ? "active" : ""}`}
            onClick={() => onChange({ result: v.key })}
          >
            {v.label}
          </button>
        ))}
      </div>

      {controls.result === "suggestions" && (
        <div className="mst-toolbar mst-filters">
          <SearchBar className="search-box mst-search" placeholder="Search…" value={search} onChange={setSearch} />
          <label className="mst-select">Category
            <select value={category ?? ""} onChange={e => onChange({ category: e.target.value || null })}>
              <option value="">All</option>
              {overview.categories.map(c => <option key={c.category} value={c.category}>{c.category}</option>)}
            </select>
          </label>
          <label className="mst-select">Progress
            <select value={controls.progress} onChange={e => onChange({ progress: e.target.value as ProgressFilter })}>
              {PROGRESS_OPTIONS.map(p => <option key={p.key} value={p.key}>{p.label}</option>)}
            </select>
          </label>
          <label className="mst-select">Availability
            <select value={controls.availability} onChange={e => onChange({ availability: e.target.value as AvailabilityFilter })}>
              {AVAILABILITY_OPTIONS.map(a => <option key={a.key} value={a.key}>{a.label}</option>)}
            </select>
          </label>
          <label className="mst-select">Sort
            <select value={controls.sort} onChange={e => onChange({ sort: e.target.value as Sort })}>
              {SORT_OPTIONS.map(s => <option key={s.key} value={s.key}>{s.label}</option>)}
            </select>
          </label>
          {isFiltered && (
            <button className="fchip fchip-reset" onClick={() => {
              setSearch("");
              onChange({ category: DEFAULT_CONTROLS.category, progress: DEFAULT_CONTROLS.progress, availability: DEFAULT_CONTROLS.availability });
            }}>
              Show All
            </button>
          )}
        </div>
      )}

      <div className="mst-body">
        {controls.result === "relics" && (
          <div className="mst-empty">Relic suggestions are not available yet.</div>
        )}
        {controls.result === "platinum" && (
          <div className="mst-empty">Platinum suggestions are not available yet.</div>
        )}
        {controls.result === "suggestions" && overview.opportunities.length === 0 && (
          <div className="mst-empty">No suggestions yet: nothing owned or building with mastery left, and no known vendor route.</div>
        )}
        {controls.result === "suggestions" && overview.opportunities.length > 0 && visible.length === 0 && (
          <div className="mst-empty">Nothing matches.</div>
        )}
        {controls.result === "suggestions" && groups.map(({ stage, items }) => (
          <section key={stage} className="mst-group" aria-label={STAGE_LABELS[stage]}>
            <div className="mst-group-header">{STAGE_LABELS[stage]} <span className="mst-count">{items.length}</span></div>
            <div className="mst-opp-list">
              {items.map(o => <OpportunityRow key={o.unique_name} opportunity={o} nowMs={nowMs} clockFormat={clockFormat} />)}
            </div>
          </section>
        ))}
      </div>
    </>
  );
}
