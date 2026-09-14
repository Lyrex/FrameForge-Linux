import { useState, useEffect, useMemo } from "react";
import ItemImg from "../ItemImg";
import SearchBar from "../shared/SearchBar";
import { MASTERY_EXCLUDE_OPTIONS } from "../constants/settings";
import { groupSources } from "./masteryGroups";
import { Progress } from "./Mastery";
import type { MasteryOverview, MasterySource, MasteryState } from "../types/mastery";

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
  const unknownTitle = steelPath ? "Steel Path clears are not read from the account yet" : "No account observation yet";
  const classNoun = MASTERY_EXCLUDE_OPTIONS.find(o => o.key === source.unobtainable)?.noun;
  return (
    <div className={`mst-item mst-${source.state}`} tabIndex={0}>
      <ItemImg imageName={source.image_name ?? undefined} fallback={<div className="img-fallback">{source.name[0]?.toUpperCase() ?? "?"}</div>} />
      <span className="mst-name">{source.name}</span>
      {steelPath && <span className="mst-mr">Steel Path</span>}
      {classNoun && <span className="mst-mr" title={source.excluded ? "Not counted toward progress; see Settings › Mastery" : undefined}>{classNoun}</span>}
      {source.mastery_req != null && source.mastery_req > 0 && (
        <span className="mst-mr" title={`Mastery Rank ${source.mastery_req} required`}>MR{source.mastery_req}</span>
      )}
      {source.remaining_mastery != null && source.remaining_mastery > 0 && (
        <span className="mst-mr" title="Remaining mastery">+{source.remaining_mastery.toLocaleString("en-US")}</span>
      )}
      <span className={`mst-rank rank-${source.state}`} title={source.state === "unknown" ? unknownTitle : undefined}>
        {source.state === "mastered" ? "✓" : rank}
      </span>
    </div>
  );
}

export default function Collection({ overview }: { overview: MasteryOverview }) {
  const [activeCategory, setActiveCategory] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [bucket, setBucket] = useState<Bucket | null>(null);

  const categories = overview.categories;
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
  // A header over the only group repeats the tab name. The whole category
  // decides this, so a search that narrows a category to one visible group
  // keeps its header instead of flickering it away while typing.
  const singleGroup = useMemo(() => category != null && groupSources(category.sources).length === 1, [category]);

  return (
    <>
      <div className="mst-tabs" role="group" aria-label="Category">
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
        {categories.length === 0 && <div className="mst-empty">Item catalogue not loaded yet.</div>}
        {category && groups.length === 0 && (
          <div className="mst-empty">{isFiltered ? "Nothing matches." : "Nothing in this category."}</div>
        )}
        {groups.map(({ group, sources }) => (
          <div key={group} className="mst-group">
            {!singleGroup && <div className="mst-group-header">{group}</div>}
            <div className="mst-group-grid">
              {sources.map(s => <SourceRow key={s.unique_name} source={s} />)}
            </div>
          </div>
        ))}
      </div>
    </>
  );
}
