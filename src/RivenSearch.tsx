import { useState, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { AuctionQuery, GradedAuction, GradedStat } from "./rivenTypes";
import "./RivenSearch.css";

// Sign (+/-) is per-roll, so the same list serves both the wanted-positives and
// the wanted-negative pickers.
const ALL_STATS = [
  "Critical Damage", "Critical Chance", "Multishot", "Base Damage",
  "Fire Rate", "Status Chance", "Toxicity", "Heat", "Electricity",
  "Cold", "Punch Through", "Reload Speed", "Magazine Size",
  "Projectile Flight Speed", "Status Duration",
  "Damage to Infested", "Damage to Grineer", "Damage to Corpus",
  "Attack Speed", "Range", "Combo Count Chance", "Initial Combo",
  "Heavy Attack Efficiency", "Slide Critical Chance",
  "Zoom", "Recoil", "Puncture", "Impact", "Slash", "Ammo Maximum",
];

const POLARITIES = ["madurai", "vazarin", "naramon"];

function verdictColor(verdict: string): string {
  if (verdict.startsWith("GREAT"))    return "var(--green)";
  if (verdict.startsWith("GOOD"))     return "#a8d8a8";
  if (verdict.startsWith("MEDIOCRE")) return "#f0c040";
  return "var(--red)";
}

function PlatIcon({ size = 12 }: { size?: number }) {
  return <img src="/platinum.webp" alt="plat" width={size} height={size} style={{ objectFit: "contain", flexShrink: 0 }} />;
}

function StatRow({ stat }: { stat: GradedStat }) {
  return (
    <div className={`rs-stat ${stat.positive ? "riven-buff" : "riven-curse"}`}>
      <span className="rs-stat-label">{stat.label}</span>
      <span className="rs-stat-value">{stat.display}</span>
      <span className="rs-stat-range">
        <span className="rs-stat-range-fill" style={{ width: `${Math.round(stat.position * 100)}%` }} />
      </span>
    </div>
  );
}

function nullableInt(s: string): number | null {
  const n = parseInt(s, 10);
  return Number.isFinite(n) ? n : null;
}

export default function RivenSearch() {
  const [weapons, setWeapons] = useState<string[]>([]);
  const [weapon, setWeapon] = useState("");
  const [positives, setPositives] = useState<string[]>([]);
  const [negative, setNegative] = useState<string | null>(null);
  const [polarity, setPolarity] = useState<string | null>(null);
  const [masteryMin, setMasteryMin] = useState("");
  const [masteryMax, setMasteryMax] = useState("");
  const [rerollsMin, setRerollsMin] = useState("");
  const [rerollsMax, setRerollsMax] = useState("");
  const [buyoutOnly, setBuyoutOnly] = useState(false);
  const [sort, setSort] = useState<"price" | "score">("price");

  const [results, setResults] = useState<GradedAuction[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    invoke<string[]>("get_riven_weapons").then(setWeapons).catch(() => setWeapons([]));
  }, []);

  // The typed text is searched as-is, so the suggestions only save typing; they
  // stop once the text already names a weapon exactly.
  const suggestions = useMemo(() => {
    const q = weapon.trim().toLowerCase();
    if (!q || weapons.includes(q)) return [];
    return weapons.filter(w => w.includes(q)).slice(0, 8);
  }, [weapon, weapons]);

  const runSearch = async () => {
    const query: AuctionQuery = {
      weapon,
      positive_stats: positives,
      negative_stat: negative,
      polarity,
      mastery_min: nullableInt(masteryMin),
      mastery_max: nullableInt(masteryMax),
      rerolls_min: nullableInt(rerollsMin),
      rerolls_max: nullableInt(rerollsMax),
      buyout_only: buyoutOnly,
    };
    setSearching(true);
    setError(null);
    try {
      setResults(await invoke<GradedAuction[]>("search_riven_auctions", { query }));
    } catch (e) {
      setResults(null);
      setError(typeof e === "string" ? e : "The market could not be reached.");
    } finally {
      setSearching(false);
    }
  };

  // An auction without a buyout price is ranked by its starting price; one with
  // neither sorts last rather than as free.
  const sorted = useMemo(() => {
    if (!results) return [];
    const price = (a: GradedAuction) => a.buyout_price ?? a.starting_price ?? Infinity;
    const score = (a: GradedAuction) => a.riven.analysis?.score ?? -1;
    return [...results].sort((a, b) =>
      sort === "price" ? price(a) - price(b) : score(b) - score(a));
  }, [results, sort]);

  const toggle = (arr: string[], v: string) =>
    arr.includes(v) ? arr.filter(x => x !== v) : [...arr, v];

  return (
    <div className="rs-tab">
      <div className="foundry-search-wrap rs-weapon-wrap">
        <input
          className="foundry-search"
          placeholder="Weapon name — leave empty to search all…"
          value={weapon}
          onChange={e => setWeapon(e.target.value)}
        />
        {suggestions.length > 0 && (
          <div className="rs-suggestions">
            {suggestions.map(w => (
              <div key={w} className="rs-suggestion"
                   onClick={() => setWeapon(w)}>
                {w.charAt(0).toUpperCase() + w.slice(1)}
              </div>
            ))}
          </div>
        )}
      </div>

      <div className="filter-bar rs-filters">
        <span className="fbar-label">Positives:</span>
        {ALL_STATS.map(s => (
          <button key={s} className={`fchip ${positives.includes(s) ? "fchip-on" : ""}`}
                  onClick={() => setPositives(p => toggle(p, s))}>{s}</button>
        ))}
      </div>

      <div className="filter-bar rs-filters">
        <span className="fbar-label">Negative:</span>
        {ALL_STATS.map(s => (
          <button key={s} className={`fchip ${negative === s ? "fchip-on" : ""}`}
                  onClick={() => setNegative(n => n === s ? null : s)}>{s}</button>
        ))}
      </div>

      <div className="filter-bar">
        <span className="fbar-label">Polarity:</span>
        {POLARITIES.map(p => (
          <button key={p} className={`fchip ${polarity === p ? "fchip-on" : ""}`}
                  onClick={() => setPolarity(cur => cur === p ? null : p)}>{p}</button>
        ))}
        <span className="fbar-sep" />
        <span className="fbar-label">MR</span>
        <input className="rs-num" type="number" min={0} max={16} placeholder="min"
               value={masteryMin} onChange={e => setMasteryMin(e.target.value)} />
        <input className="rs-num" type="number" min={0} max={16} placeholder="max"
               value={masteryMax} onChange={e => setMasteryMax(e.target.value)} />
        <span className="fbar-sep" />
        <span className="fbar-label">Rerolls</span>
        <input className="rs-num" type="number" min={0} placeholder="min"
               value={rerollsMin} onChange={e => setRerollsMin(e.target.value)} />
        <input className="rs-num" type="number" min={0} placeholder="max"
               value={rerollsMax} onChange={e => setRerollsMax(e.target.value)} />
        <span className="fbar-sep" />
        <button className={`fchip ${buyoutOnly ? "fchip-on" : ""}`}
                onClick={() => setBuyoutOnly(v => !v)}>Buyout only</button>
        <span className="fbar-sep" />
        <span className="fbar-label">Sort:</span>
        <button className={`fchip ${sort === "price" ? "fchip-on" : ""}`} onClick={() => setSort("price")}>Price</button>
        <button className={`fchip ${sort === "score" ? "fchip-on" : ""}`} onClick={() => setSort("score")}>Score</button>
        <span className="fbar-sep" />
        <button className="fchip fchip-reset" onClick={runSearch} disabled={searching}>
          {searching ? "Searching…" : "Search"}
        </button>
      </div>

      <div className="rs-results">
        {error && (
          <div className="rs-message rs-message-error">
            <strong>The search failed.</strong>
            <span>{error}</span>
            <button className="fchip fchip-reset" onClick={runSearch}>Try again</button>
          </div>
        )}

        {!error && results === null && (
          <div className="rs-message">Pick a weapon and the stats you want, then hit Search.</div>
        )}

        {!error && results !== null && results.length === 0 && (
          <div className="rs-message">
            <strong>No auctions match these filters.</strong>
            <span>Nothing is wrong — try dropping a stat or widening the MR and reroll ranges.</span>
          </div>
        )}

        {!error && sorted.length > 0 && (
          <div className="rivens-list">
            {sorted.map(a => {
              const analysis = a.riven.analysis;
              return (
                <div key={a.auction_id} className="riven-card">
                  <div className="riven-card-header">
                    <span className="riven-weapon">
                      {a.riven.weapon_name ?? a.riven.mod_name}
                      <span className="riven-mod-name"> {a.riven.mod_name}</span>
                    </span>
                    {analysis
                      ? <span className="rs-verdict" style={{ color: verdictColor(analysis.verdict) }}>
                          {analysis.verdict} · {Math.round(analysis.score * 100)}%
                        </span>
                      : <span className="rs-verdict rs-no-grade">no grading data</span>}
                    <button className="riven-sell-btn" onClick={() => openUrl(a.url)}>Open ↗</button>
                  </div>

                  <div className="riven-card-meta">
                    {a.buyout_price !== null && <span className="rs-price"><PlatIcon />{a.buyout_price} buyout</span>}
                    {a.starting_price !== null && <span>start {a.starting_price}</span>}
                    {a.top_bid !== null && <span>top bid {a.top_bid}</span>}
                    <span>{a.seller_name} <span className={`rs-dot rs-dot-${a.seller_status}`} />{a.seller_status}</span>
                    {a.riven.polarity && <span>{a.riven.polarity}</span>}
                    {a.riven.mastery_level !== null && <span>MR {a.riven.mastery_level}</span>}
                    <span>rank {a.riven.mod_rank}</span>
                    <span>{a.riven.rerolls} rerolls</span>
                  </div>

                  <div className="riven-stats rs-stats">
                    {[...a.riven.buffs, ...a.riven.curses].map((s, i) => <StatRow key={i} stat={s} />)}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
