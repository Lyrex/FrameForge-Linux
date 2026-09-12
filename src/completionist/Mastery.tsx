import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import ItemImg from "../ItemImg";
import SearchBar from "../shared/SearchBar";
import "./Mastery.css";
import { TAURI_COMMANDS } from "../constants/tauri";
import { groupSources } from "./masteryGroups";
import type { InventoryItem } from "../types/items";
import type { MasteryCounts, MasteryOverview, MasterySource, MasteryState } from "../types/mastery";

const BUCKETS: { state: MasteryState; label: string }[] = [
  { state: "mastered", label: "Mastered" },
  { state: "partial",  label: "Partial" },
  { state: "missing",  label: "Missing" },
  { state: "unknown",  label: "Unknown" },
];

function SourceRow({ source }: { source: MasterySource }) {
  const rank = source.earned_rank == null ? "?" : `R${source.earned_rank}/${source.cap}`;
  return (
    <div className={`mst-item mst-${source.state}`}>
      <ItemImg imageName={source.image_name ?? undefined} fallback={<div className="img-fallback">{source.name[0]?.toUpperCase() ?? "?"}</div>} />
      <span className="mst-name">{source.name}</span>
      {source.mastery_req != null && source.mastery_req > 0 && (
        <span className="mst-mr" title={`Mastery Rank ${source.mastery_req} required`}>MR{source.mastery_req}</span>
      )}
      <span className={`mst-rank rank-${source.state}`} title={source.state === "unknown" ? "No account observation yet" : undefined}>
        {source.state === "mastered" ? "✓" : rank}
      </span>
    </div>
  );
}

function Progress({ counts, label }: { counts: MasteryCounts; label: string }) {
  return (
    <div className="mst-progress-wrap" title={`${label}: ${counts.mastered} mastered, ${counts.partial} partial, ${counts.missing} missing, ${counts.unknown} unknown`}>
      <div className="mst-progress-bar">
        <div className="mst-progress-fill" style={{ width: counts.total > 0 ? `${(counts.mastered / counts.total) * 100}%` : "0%" }} />
      </div>
      <span className="mst-progress-label">
        {label} {counts.mastered} / {counts.total} mastered
        {counts.unknown > 0 && <> · {counts.unknown} unknown</>}
      </span>
    </div>
  );
}

interface Props {
  /** Only a change signal: the overview itself comes from the backend. */
  inventory: Record<string, InventoryItem>;
  refreshKey: number;
}

export default function Mastery({ inventory, refreshKey }: Props) {
  const [overview, setOverview] = useState<MasteryOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activeCategory, setActiveCategory] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [bucket, setBucket] = useState<MasteryState | null>(null);

  useEffect(() => {
    let stale = false;
    invoke<MasteryOverview>(TAURI_COMMANDS.GET_MASTERY_OVERVIEW)
      .then(o => { if (!stale) { setOverview(o); setError(null); } })
      .catch(e => { if (!stale) setError(String(e)); });
    return () => { stale = true; };
  }, [inventory, refreshKey]);

  const categories = overview?.categories ?? [];
  const category = categories.find(c => c.category === activeCategory) ?? categories[0];

  useEffect(() => { setSearch(""); }, [category?.category]);

  const groups = useMemo(() => {
    if (!category) return [];
    const q = search.toLowerCase();
    const visible = category.sources.filter(s =>
      (bucket == null || s.state === bucket) && (!q || s.name.toLowerCase().includes(q)));
    return groupSources(visible);
  }, [category, search, bucket]);

  const isFiltered = search !== "" || bucket != null;

  return (
    <div className="mst-root">
      <div className="mst-tabs" role="group" aria-label="Equipment category">
        {categories.map(c => (
          <button
            key={c.category}
            aria-pressed={c.category === category?.category}
            className={`mst-tab ${c.category === category?.category ? "active" : ""}`}
            onClick={() => setActiveCategory(c.category)}
          >
            {c.category}
          </button>
        ))}
      </div>

      <div className="mst-toolbar">
        <SearchBar className="search-box mst-search" placeholder="Search…" value={search} onChange={setSearch} />
        {category && <Progress counts={category.counts} label={category.category} />}
        {overview && <Progress counts={overview.counts} label="All" />}
      </div>
      <div className="mst-toolbar mst-buckets" role="group" aria-label="Progress filter">
        {BUCKETS.map(b => {
          const n = category?.counts[b.state] ?? 0;
          if (b.state === "unknown" && n === 0) return null;
          return (
            <button
              key={b.state}
              className={`mst-filter-btn ${bucket === b.state ? "active" : ""}`}
              aria-pressed={bucket === b.state}
              onClick={() => setBucket(v => v === b.state ? null : b.state)}
            >
              {b.label} <span className="mst-count">{n}</span>
            </button>
          );
        })}
        {isFiltered && (
          <button className="fchip fchip-reset" onClick={() => { setSearch(""); setBucket(null); }}>
            Show All
          </button>
        )}
      </div>

      <div className="mst-body">
        {error && <div className="mst-empty">Mastery overview unavailable: {error}</div>}
        {!error && !overview && <div className="mst-empty">Loading mastery…</div>}
        {overview && categories.length === 0 && <div className="mst-empty">Item catalogue not loaded yet.</div>}
        {category && groups.length === 0 && (
          <div className="mst-empty">{isFiltered ? "Nothing matches." : "No equipment in this category."}</div>
        )}
        {groups.map(({ group, sources }) => (
          <div key={group} className="mst-group">
            <div className="mst-group-header">{group}</div>
            <div className="mst-group-grid">
              {sources.map(s => <SourceRow key={s.unique_name} source={s} />)}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
