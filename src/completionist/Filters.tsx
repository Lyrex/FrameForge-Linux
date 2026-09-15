import SearchBar from "../shared/SearchBar";
import {
  activePreset, AVAILABILITY_OPTIONS, COMPARISON_OPTIONS, DEFAULT_CONTROLS, PRESETS, PROGRESS_OPTIONS, SORT_OPTIONS,
  type AvailabilityFilter, type Comparison, type MasteryControls, type ProgressFilter, type ResultView, type Sort,
} from "./suggestions";
import type { MasteryOverview } from "../types/mastery";

interface Props {
  overview: MasteryOverview;
  controls: MasteryControls;
  onChange: (patch: Partial<MasteryControls>) => void;
  /** The plan keeps its own result view apart from `controls.result`, so the caller says which one the filters narrow. */
  result: ResultView;
  search: string;
  onSearch: (value: string) => void;
}

export default function Filters({ overview, controls, onChange, result, search, onSearch }: Props) {
  const category = overview.categories.some(c => c.category === controls.category) ? controls.category : null;
  const isFiltered = search !== "" || (!controls.easy && (category != null || controls.progress !== "all" || controls.availability !== "all"));
  const preset = activePreset(controls);
  return (
    <div className="mst-toolbar mst-filters">
      {!controls.easy && (
        <label className="mst-select" title={PRESETS.find(p => p.key === preset)?.title}>Preset
          <select value={preset ?? ""} onChange={e => { const p = PRESETS.find(x => x.key === e.target.value); if (p) onChange(p.patch); }}>
            <option value="" disabled>Custom</option>
            {PRESETS.map(p => <option key={p.key} value={p.key} title={p.title}>{p.label}</option>)}
          </select>
        </label>
      )}
      {result === "platinum" && (
        <label className="mst-select">Rank by
          <select value={controls.comparison} onChange={e => onChange({ comparison: e.target.value as Comparison })}>
            {COMPARISON_OPTIONS.map(c => <option key={c.key} value={c.key}>{c.label}</option>)}
          </select>
        </label>
      )}
      <SearchBar className="search-box mst-search" placeholder="Search…" value={search} onChange={onSearch} />
      {isFiltered && (
        <button className="fchip fchip-reset" onClick={() => {
          onSearch("");
          onChange({ category: DEFAULT_CONTROLS.category, progress: DEFAULT_CONTROLS.progress, availability: DEFAULT_CONTROLS.availability });
        }}>
          Show All
        </button>
      )}
      {!controls.easy && (
        <>
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
          {result !== "platinum" && (
            <label className="mst-select">Sort
              <select value={controls.sort} onChange={e => onChange({ sort: e.target.value as Sort })}>
                {SORT_OPTIONS.map(s => <option key={s.key} value={s.key}>{s.label}</option>)}
              </select>
            </label>
          )}
          {result === "suggestions" && (
            <label className="mst-select">
              <input type="checkbox" checked={controls.hideRelics} onChange={e => onChange({ hideRelics: e.target.checked })} />
              Hide relic routes
            </label>
          )}
        </>
      )}
    </div>
  );
}
