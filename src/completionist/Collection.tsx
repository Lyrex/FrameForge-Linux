import { useState, useEffect, useMemo } from "react";
import ItemImg from "../ItemImg";
import { formatCount } from "../lib/formatters.ts";
import SearchBar from "../shared/SearchBar";
import { MASTERY_EXCLUDE_OPTIONS } from "../constants/settings";
import { matchesSearch, sectionCategories } from "./masteryGroups";
import { progressText } from "./topBar";
import type { MasteryCounts, MasteryOverview, MasterySource, MasteryState } from "../types/mastery";

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
  const steelPath = source.node?.mode === "steel_path";
  const rank = source.earned_rank == null ? "?" : source.node ? "—" : `R${source.earned_rank}/${source.cap}`;
  const classNoun = MASTERY_EXCLUDE_OPTIONS.find(o => o.key === source.unobtainable)?.noun;
  return (
    <div className={`mst-item mst-${source.state}`} tabIndex={0}>
      <ItemImg imageName={source.image_name ?? undefined} fallback={<div className="img-fallback">{source.name[0]?.toUpperCase() ?? "?"}</div>} />
      <span className="mst-name">{source.name}</span>
      {source.needed_for.length > 0 && <span className="mst-mr">Needed for {source.needed_for.join(", ")}</span>}
      {steelPath && <span className="mst-mr">Steel Path</span>}
      {classNoun && <span className="mst-mr" title={source.excluded ? "Not counted toward progress; see Settings › Mastery" : undefined}>{classNoun}</span>}
      {source.mastery_req != null && source.mastery_req > 0 && (
        <span className="mst-mr" title={`Mastery Rank ${source.mastery_req} required`}>MR{source.mastery_req}</span>
      )}
      {source.remaining_mastery != null && source.remaining_mastery > 0 && (
        <span className="mst-mr" title="Remaining mastery">+{formatCount(source.remaining_mastery)}</span>
      )}
      <span className={`mst-rank rank-${source.state}`} title={source.state === "unknown" ? "No scan yet" : undefined}>
        {source.state === "mastered" ? "✓" : rank}
      </span>
    </div>
  );
}

const ALL = "All";

export default function Collection({ overview }: { overview: MasteryOverview }) {
  const [activeCategory, setActiveCategory] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [bucket, setBucket] = useState<Bucket | null>(null);

  const categories = overview.categories;
  const all = activeCategory === ALL;
  const category = all ? undefined : categories.find(c => c.category === activeCategory) ?? categories[0];
  const active = all ? ALL : category?.category;
  const counts = all ? overview.counts : category?.counts;
  const tabs = categories.length > 0 ? [ALL, ...categories.map(c => c.category)] : [];

  useEffect(() => { setSearch(""); }, [active]);

  const sections = useMemo(() => {
    const shown = all ? categories : category ? [category] : [];
    return sectionCategories(shown, s => inBucket(s, bucket) && matchesSearch(s, search));
  }, [all, categories, category, search, bucket]);

  const isFiltered = search !== "" || bucket != null;

  return (
    <>
      <div className="mst-tabs" role="group" aria-label="Category">
        {tabs.map(tab => (
          <button
            key={tab}
            aria-pressed={tab === active}
            className={`mst-tab ${tab === active ? "active" : ""}`}
            onClick={() => setActiveCategory(tab)}
          >
            {tab}
          </button>
        ))}
      </div>

      <div className="mst-toolbar">
        <SearchBar className="search-box mst-search" placeholder="Search…" value={search} onChange={setSearch} />
        {category && <Progress counts={category.counts} label={category.category} />}
      </div>
      <div className="mst-toolbar mst-buckets" role="group" aria-label="Progress filter">
        {BUCKETS.map(b => {
          const n = counts?.[b.bucket] ?? 0;
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
        {categories.length === 0 && <div className="mst-empty">Item catalogue not loaded yet.</div>}
        {categories.length > 0 && sections.length === 0 && (
          <div className="mst-empty">{isFiltered ? "Nothing matches." : "Nothing in this category."}</div>
        )}
        {sections.map(({ key, header, sources }) => (
          <div key={key} className="mst-group">
            {header && <div className="mst-group-header">{header}</div>}
            <div className="mst-group-grid">
              {sources.map(s => <SourceRow key={s.unique_name} source={s} />)}
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
