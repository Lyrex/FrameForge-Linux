import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { verdictColor } from "./rivenTypes";
import type { GradedRiven, GradedStat } from "./rivenTypes";
import { formatChallengeName } from "./MarketHelper";
import polMadurai  from "./assets/polarity/madurai.svg";
import polVazarin  from "./assets/polarity/vazarin.svg";
import polNaramon  from "./assets/polarity/naramon.svg";
import polZenurik  from "./assets/polarity/zenurik.svg";
import polUnairu   from "./assets/polarity/unairu.svg";
import polPenjaga  from "./assets/polarity/penjaga.svg";
import polUmbra    from "./assets/polarity/umbra.svg";
import "./RivenOwned.css";

const POLARITY_DISPLAY: Record<string, { icon: string; name: string }> = {
  AP_ATTACK:  { icon: polMadurai,  name: "Madurai"  },
  AP_DEFENSE: { icon: polVazarin,  name: "Vazarin"  },
  AP_TACTIC:  { icon: polNaramon,  name: "Naramon"  },
  AP_WARD:    { icon: polUnairu,   name: "Unairu"   },
  AP_POWER:   { icon: polZenurik,  name: "Zenurik"  },
  AP_UMBRA:   { icon: polUmbra,    name: "Umbra"    },
  AP_PRECEPT: { icon: polPenjaga,  name: "Penjaga"  },
};

// The analyzer's verdicts read "GREAT ROLL — Consider keeping"; the leading word
// is what filters and badges work with.
const VERDICT_KEYS = ["GREAT", "GOOD", "MEDIOCRE", "BAD"];
const verdictKey = (verdict: string) => verdict.split(" ")[0];
const verdictRank = (r: GradedRiven) =>
  r.analysis ? VERDICT_KEYS.indexOf(verdictKey(r.analysis.verdict)) : VERDICT_KEYS.length;

const REROLL_BUCKETS: { label: string; test: (n: number) => boolean }[] = [
  { label: "Unrolled", test: n => n === 0 },
  { label: "1–5",      test: n => n >= 1 && n <= 5 },
  { label: "6–15",     test: n => n >= 6 && n <= 15 },
  { label: "16+",      test: n => n >= 16 },
];

function StatRow({ stat }: { stat: GradedStat }) {
  const pct = Math.min(100, Math.round(stat.position * 100));
  return (
    <div className="rown-stat">
      <span className={`riven-stat ${stat.positive ? "riven-buff" : "riven-curse"}`}>
        {stat.display} {stat.label}
      </span>
      {stat.analyzer_name === null && <span className="rown-unscored">unscored</span>}
      <span className="rown-bound">{stat.min_display}</span>
      <div className="rown-range">
        <div className="rown-range-marker" style={{ left: `${pct}%` }} />
      </div>
      <span className="rown-bound">{stat.max_display}</span>
      <span className="rown-pos">{pct}%</span>
    </div>
  );
}

export default function RivenOwned({ onOpenInAnalyzer, onSell, canSell }: {
  onOpenInAnalyzer: (riven: GradedRiven) => void;
  onSell: (riven: GradedRiven) => void;
  /** Posting needs a warframe.market session; without one the button only explains itself. */
  canSell: boolean;
}) {
  const [rivens,  setRivens]  = useState<GradedRiven[]>([]);
  const [error,   setError]   = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const [search,   setSearch]   = useState("");
  const [cats,     setCats]     = useState<string[]>([]);
  const [verdicts, setVerdicts] = useState<string[]>([]);
  const [rerolls,  setRerolls]  = useState<string[]>([]);
  const [sortMode, setSortMode] = useState<"score" | "worst" | "verdict">("score");

  useEffect(() => {
    const load = () =>
      invoke<GradedRiven[]>("grade_owned_rivens")
        .then(rs => { setRivens(rs); setError(null); })
        .catch(e => setError(String(e)))
        .finally(() => setLoading(false));
    load();
    // The scanner has no riven-specific event; a fresh roll arrives with the inventory.
    const unlisten = listen("inventory-update", load);
    return () => { unlisten.then(fn => fn()); };
  }, []);

  const categories = useMemo(
    () => [...new Set(rivens.map(r => r.category))].sort(),
    [rivens],
  );

  const toggle = (list: string[], v: string) =>
    list.includes(v) ? list.filter(x => x !== v) : [...list, v];

  const matches = (r: GradedRiven) => {
    const q = search.trim().toLowerCase();
    if (q && !(r.weapon_name ?? "").toLowerCase().includes(q)
          && !r.mod_name.toLowerCase().includes(q)) return false;
    if (cats.length && !cats.includes(r.category)) return false;
    if (verdicts.length && (!r.analysis || !verdicts.includes(verdictKey(r.analysis.verdict)))) return false;
    if (rerolls.length && !REROLL_BUCKETS.some(b => rerolls.includes(b.label) && b.test(r.rerolls))) return false;
    return true;
  };

  const visible = rivens.filter(matches);
  const unlocked = visible.filter(r => r.state === "unlocked");
  const revealed = visible.filter(r => r.state === "revealed");
  const unrevealed = visible.filter(r => r.state === "unrevealed");

  // Ungraded rolls sort last in every mode: no data is not a low score.
  const sorted = [...unlocked].sort((a, b) => {
    if (!a.analysis || !b.analysis) return (a.analysis ? 0 : 1) - (b.analysis ? 0 : 1);
    if (sortMode === "verdict") return verdictRank(a) - verdictRank(b) || b.analysis.score - a.analysis.score;
    if (sortMode === "worst")   return a.analysis.score - b.analysis.score;
    return b.analysis.score - a.analysis.score;
  });

  const resetFilters = () => { setSearch(""); setCats([]); setVerdicts([]); setRerolls([]); };

  if (loading) return <div className="market-placeholder">Grading rivens…</div>;
  if (error) return (
    <div className="market-placeholder">
      <p>Could not grade owned rivens: {error}</p>
    </div>
  );
  if (rivens.length === 0) return (
    <div className="market-placeholder">
      <p>No rivens found. Warframe must have run at least once with the scanner active.</p>
    </div>
  );

  return (
    <div className="rivens-tab">
      <div className="filter-bar" style={{ border: "none", padding: 0 }}>
        <input className="foundry-search" style={{ width: 180 }} placeholder="Search weapon…"
          value={search} onChange={e => setSearch(e.target.value)} />
        {categories.map(c => (
          <button key={c} className={`fchip ${cats.includes(c) ? "fchip-on" : ""}`}
            onClick={() => setCats(toggle(cats, c))}>{c}</button>
        ))}
        <span className="fbar-sep" />
        {VERDICT_KEYS.map(v => (
          <button key={v} className={`fchip ${verdicts.includes(v) ? "fchip-on" : ""}`}
            onClick={() => setVerdicts(toggle(verdicts, v))}>{v}</button>
        ))}
        <span className="fbar-sep" />
        {REROLL_BUCKETS.map(b => (
          <button key={b.label} className={`fchip ${rerolls.includes(b.label) ? "fchip-on" : ""}`}
            onClick={() => setRerolls(toggle(rerolls, b.label))}>{b.label}</button>
        ))}
        <span className="fbar-sep" />
        <span className="fbar-label">Sort:</span>
        <button className={`fchip ${sortMode === "score"   ? "fchip-on" : ""}`} onClick={() => setSortMode("score")}>Best first</button>
        <button className={`fchip ${sortMode === "worst"   ? "fchip-on" : ""}`} onClick={() => setSortMode("worst")}>Worst first</button>
        <button className={`fchip ${sortMode === "verdict" ? "fchip-on" : ""}`} onClick={() => setSortMode("verdict")}>Verdict</button>
        <span className="fbar-sep" />
        <button className="fchip fchip-reset" onClick={resetFilters}>Show All</button>
      </div>

      {sorted.length > 0 && (
        <section>
          <div className="rivens-section-header">Graded ({sorted.length})</div>
          <div className="rivens-list">
            {sorted.map(r => (
              <div key={r.item_id} className="riven-card">
                <div className="riven-card-header">
                  {r.analysis
                    ? <span className="rown-verdict" style={{ color: verdictColor(r.analysis.verdict), borderColor: verdictColor(r.analysis.verdict) }}>
                        {verdictKey(r.analysis.verdict)} {Math.round(r.analysis.score * 100)}%
                      </span>
                    : <span className="rown-verdict rown-ungraded">NO GRADING DATA</span>}
                  <span className="riven-weapon">
                    {r.weapon_name ?? "Unknown"}
                    {r.mod_name && <span className="riven-mod-name"> {r.mod_name.replace(/^./, c => c.toUpperCase())}</span>}
                  </span>
                  <button className="riven-sell-btn" onClick={() => onOpenInAnalyzer(r)}>Analyzer</button>
                  <button className="riven-sell-btn"
                    disabled={!canSell || !r.weapon_name}
                    title={!r.weapon_name ? "The weapon is unknown, so the auction cannot name it"
                         : canSell ? "Post auction on warframe.market"
                         : "Log in to warframe.market first (Market → Trading tab)"}
                    onClick={() => onSell(r)}>Sell ↗</button>
                </div>
                <div className="riven-card-meta">
                  <span>{r.category}</span>
                  <span>MR {r.mastery_level ?? "?"}</span>
                  <span>Rank {r.mod_rank}</span>
                  <span>{r.disposition.toFixed(2)}x</span>
                  <span>{r.rerolls} roll{r.rerolls !== 1 ? "s" : ""}</span>
                  {r.polarity && POLARITY_DISPLAY[r.polarity] && (
                    <span className="riven-polarity">
                      <img src={POLARITY_DISPLAY[r.polarity].icon} className="polarity-icon" alt={POLARITY_DISPLAY[r.polarity].name} />
                      {" "}{POLARITY_DISPLAY[r.polarity].name}
                    </span>
                  )}
                </div>
                <div className="rown-stats">
                  {[...r.buffs, ...r.curses].map(s => <StatRow key={s.tag + s.display} stat={s} />)}
                </div>
                {!r.analysis && (
                  <div className="rown-note">This weapon is not in the wanted-stats database, so the roll cannot be scored.</div>
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      {revealed.length > 0 && (
        <section>
          <div className="rivens-section-header">Revealed, still locked ({revealed.length})</div>
          <div className="rivens-list">
            {revealed.map(r => (
              <div key={r.item_id} className="riven-card riven-revealed">
                <div className="riven-card-header">
                  <span className="riven-weapon">{r.category} Riven Mod</span>
                  <span className="riven-meta riven-challenge">{formatChallengeName(r.challenge, r.complication)}</span>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {unrevealed.length > 0 && (
        <section>
          <div className="rivens-section-header">
            Unrevealed ({unrevealed.reduce((n, r) => n + r.count, 0)})
          </div>
          <div className="rivens-list">
            {unrevealed.map(r => (
              <div key={r.item_id} className="riven-card riven-veiled">
                <div className="riven-card-header">
                  <span className="riven-weapon">{r.category} Riven Mod</span>
                  {r.count > 1 && <span className="riven-meta">×{r.count}</span>}
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {visible.length === 0 && (
        <div className="market-placeholder">No riven matches these filters.</div>
      )}
    </div>
  );
}
