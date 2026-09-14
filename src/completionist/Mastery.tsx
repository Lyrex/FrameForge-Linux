import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import ItemImg from "../ItemImg";
import SearchBar from "../shared/SearchBar";
import "./Mastery.css";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import { MASTERY_EXCLUDE_OPTIONS } from "../constants/settings";
import { groupSources } from "./masteryGroups";
import { formatAge } from "../lib/formatters";
import { fmtClock, type ClockFormat } from "../lib/clockFormat";
import type { InventoryItem } from "../types/items";
import type { MasteryCounts, MasteryOverview, MasteryProvenance, MasterySource, MasteryState, Provenance } from "../types/mastery";

type Bucket = MasteryState | "unobtainable";

const BUCKETS: { bucket: Bucket; label: string }[] = [
  { bucket: "mastered",     label: "Mastered" },
  { bucket: "partial",      label: "Partial" },
  { bucket: "missing",      label: "Missing" },
  { bucket: "unknown",      label: "Unknown" },
  { bucket: "unobtainable", label: "Unobtainable" },
];

function inBucket(source: MasterySource, bucket: Bucket | null): boolean {
  if (bucket === "unobtainable") return source.excluded;
  return !source.excluded && (bucket == null || source.state === bucket);
}

function SourceRow({ source }: { source: MasterySource }) {
  const rank = source.earned_rank == null ? "?" : `R${source.earned_rank}/${source.cap}`;
  const classNoun = MASTERY_EXCLUDE_OPTIONS.find(o => o.key === source.unobtainable)?.noun;
  return (
    <div className={`mst-item mst-${source.state}`}>
      <ItemImg imageName={source.image_name ?? undefined} fallback={<div className="img-fallback">{source.name[0]?.toUpperCase() ?? "?"}</div>} />
      <span className="mst-name">{source.name}</span>
      {classNoun && <span className="mst-mr" title={source.excluded ? "Not counted toward progress; see Settings › Mastery" : undefined}>{classNoun}</span>}
      {source.mastery_req != null && source.mastery_req > 0 && (
        <span className="mst-mr" title={`Mastery Rank ${source.mastery_req} required`}>MR{source.mastery_req}</span>
      )}
      <span className={`mst-rank rank-${source.state}`} title={source.state === "unknown" ? "No account observation yet" : undefined}>
        {source.state === "mastered" ? "✓" : rank}
      </span>
    </div>
  );
}

const SOURCE_KINDS: { key: keyof MasteryProvenance; label: string }[] = [
  { key: "equipment",  label: "Equipment" },
  { key: "intrinsics", label: "Intrinsics" },
  { key: "nodes",      label: "Nodes" },
  { key: "junctions",  label: "Junctions" },
];

function pillText(label: string, { state, observed_at }: Provenance, now: number, clockFormat: ClockFormat): { detail: string; title: string } {
  if (state === "confirmed" && observed_at != null) {
    const day = new Date(observed_at * 1000).toLocaleDateString(navigator.language, { month: "short", day: "numeric" });
    return { detail: formatAge(observed_at, now), title: `${label}: observed ${day}, ${fmtClock(observed_at, clockFormat)}` };
  }
  if (state === "unconfirmed") return { detail: "Unconfirmed", title: `${label}: carried over from a cache with no observation time` };
  return { detail: "Unknown", title: `${label}: no observation yet` };
}

function ProvenancePill({ label, provenance, now, clockFormat }: { label: string; provenance: Provenance; now: number; clockFormat: ClockFormat }) {
  const { state } = provenance;
  const { detail, title } = pillText(label, provenance, now, clockFormat);
  return (
    <span className={`mst-pill mst-pill-${state}`} title={title}>
      <span className="mst-pill-kind">{label}</span> {detail}
    </span>
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
        {counts.unobtainable > 0 && <> · {counts.unobtainable} unobtainable</>}
      </span>
    </div>
  );
}

interface Props {
  /** Only a change signal: the overview itself comes from the backend. */
  inventory: Record<string, InventoryItem>;
  refreshKey: number;
  clockFormat: ClockFormat;
}

export default function Mastery({ inventory, refreshKey, clockFormat }: Props) {
  const [overview, setOverview] = useState<MasteryOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activeCategory, setActiveCategory] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [bucket, setBucket] = useState<Bucket | null>(null);
  // Provenance changes (a re-observation, a player switch) and the exclusion
  // settings leave the inventory prop untouched, so the backend announces them.
  const [observationKey, setObservationKey] = useState(0);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  useEffect(() => {
    const refetch = () => setObservationKey(k => k + 1);
    const unlisten = Promise.all([
      listen(TAURI_EVENTS.MASTERY_UPDATE, refetch),
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

  const categories = overview?.categories ?? [];
  const category = categories.find(c => c.category === activeCategory) ?? categories[0];

  useEffect(() => { setSearch(""); }, [category?.category]);

  const groups = useMemo(() => {
    if (!category) return [];
    const q = search.toLowerCase();
    const visible = category.sources.filter(s =>
      inBucket(s, bucket) && (!q || s.name.toLowerCase().includes(q)));
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
        {overview && SOURCE_KINDS.map(kind => (
          <ProvenancePill key={kind.key} label={kind.label} provenance={overview.provenance[kind.key]} now={now} clockFormat={clockFormat} />
        ))}
      </div>
      <div className="mst-toolbar mst-buckets" role="group" aria-label="Progress filter">
        {BUCKETS.map(b => {
          const n = category?.counts[b.bucket] ?? 0;
          if ((b.bucket === "unknown" || b.bucket === "unobtainable") && n === 0) return null;
          return (
            <button
              key={b.bucket}
              className={`mst-filter-btn ${bucket === b.bucket ? "active" : ""}`}
              aria-pressed={bucket === b.bucket}
              onClick={() => setBucket(v => v === b.bucket ? null : b.bucket)}
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
