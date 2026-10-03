import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { checkRivenNow } from "../lib/rivenWindow";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import type { RivenAnalysis, RivenStat, SavedRiven } from "../types/rivens";
import type { AnalyzeRivenArgs, SaveRivenRollArgs } from "../types/tauri";

// ── Tailwind class constants (formerly RivenAnalyzer.css) ─────────────────────

const RA_ANALYZER =
  "flex flex-col gap-[10px] px-[14px] py-[12px] h-full min-h-0 overflow-y-auto";
const RA_HEADER = "flex items-center gap-[8px] shrink-0";
const RA_TITLE = "text-[13px] font-bold text-foreground";
const RA_DB_STATUS = "ml-auto text-[10px] text-muted";
const RA_CREDIT =
  "bg-transparent border-0 p-0 text-[10px] text-muted cursor-pointer opacity-60 transition-[opacity,color] duration-100 whitespace-nowrap hover:opacity-100 hover:text-accent";
const RA_CHECK_BTN =
  "bg-[rgba(56,139,253,0.15)] border border-[rgba(56,139,253,0.4)] text-[#58a6ff] text-[12px] font-semibold cursor-pointer px-[10px] py-[4px] rounded-[5px] transition-[background] duration-150 hover:bg-[rgba(56,139,253,0.28)]";
const RA_REFRESH_BTN =
  "bg-transparent border-0 text-muted text-[14px] cursor-pointer px-[2px] py-0 transition-[color] duration-100 hover:text-foreground";

const RA_WEAPON_WRAP = "relative shrink-0";
const RA_WEAPON_INPUT =
  "w-full bg-[rgba(0,0,0,0.2)] border border-[rgba(48,54,61,0.8)] rounded-[5px] text-foreground text-[13px] font-semibold px-[10px] py-[7px] outline-none focus:border-accent";
const RA_SUGGESTIONS =
  "absolute top-full left-0 right-0 bg-surface border border-t-0 border-[rgba(48,54,61,0.8)] rounded-b-[5px] z-10 max-h-[220px] overflow-y-auto";
const RA_SUGGESTION =
  "px-[10px] py-[6px] text-[12px] text-foreground cursor-pointer transition-[background] duration-100 hover:bg-[rgba(56,139,253,0.12)]";

const RA_SECTION_LABEL =
  "text-[10px] font-bold uppercase tracking-[0.04em] text-muted shrink-0";
const RA_OPTIONAL = "font-normal normal-case tracking-normal italic";
const RA_STAT_GRID = "flex flex-wrap gap-[4px] shrink-0";
const RA_STAT_BTN =
  "bg-[rgba(255,255,255,0.05)] border border-[rgba(48,54,61,0.6)] text-muted text-[11px] px-[9px] py-[3px] rounded-[4px] cursor-pointer whitespace-nowrap transition-[background,color,border-color] duration-100 hover:bg-[rgba(255,255,255,0.1)] hover:text-foreground";
const RA_STAT_BTN_SELECTED =
  "bg-[rgba(255,255,255,0.05)] border border-[rgba(48,54,61,0.6)] text-muted text-[11px] px-[9px] py-[3px] rounded-[4px] cursor-pointer whitespace-nowrap transition-[background,color,border-color] duration-100 hover:bg-[rgba(255,255,255,0.1)] hover:text-foreground bg-[rgba(63,185,80,0.15)]! border-[var(--green)]! text-[var(--green)]!";

const RA_VERDICT = "text-[16px] font-bold tracking-[0.01em]";
const RA_STATS_BREAKDOWN = "flex flex-col gap-[3px]";
const RA_STAT_ROW = "flex items-center gap-[8px] text-[12px] py-[3px]";
const RA_STAT_ICON = "w-[14px] text-center shrink-0 text-[11px]";
const RA_STAT_TAG =
  "ml-auto text-[10px] px-[6px] py-[1px] rounded-[3px] shrink-0";
const STAT_TONE: Record<string, { row: string; tag: string }> = {
  good: {
    row: RA_STAT_ROW + " text-success",
    tag: RA_STAT_TAG + " bg-[rgba(63,185,80,0.12)] text-success",
  },
  miss: {
    row: RA_STAT_ROW + " text-muted",
    tag: RA_STAT_TAG + " bg-[rgba(255,255,255,0.06)] text-muted",
  },
  safe: {
    row: RA_STAT_ROW + " text-[#6eb6ff]",
    tag: RA_STAT_TAG + " bg-[rgba(110,182,255,0.12)] text-[#6eb6ff]",
  },
  bad: {
    row: RA_STAT_ROW + " text-danger",
    tag: RA_STAT_TAG + " bg-[rgba(248,81,73,0.12)] text-danger",
  },
};

const RA_NOTES =
  "text-[11px] text-muted leading-[1.5] border-t border-t-[rgba(48,54,61,0.4)] pt-[8px]";
const RA_NEXT_ROLL =
  "bg-[rgba(56,139,253,0.12)] border border-accent text-accent text-[12px] font-semibold px-[16px] py-[7px] rounded-[5px] cursor-pointer self-start transition-[background] duration-100 shrink-0 hover:bg-[rgba(56,139,253,0.25)]";

const RA_VALUE_INPUTS =
  "bg-[rgba(0,0,0,0.2)] border border-[rgba(48,54,61,0.5)] rounded-[6px] p-[10px] flex flex-col gap-[6px] shrink-0";
const RA_VALUE_ROW = "flex items-center gap-[8px]";
const RA_VALUE_LABEL = "flex-1 text-[12px] text-foreground min-w-[160px]";
const RA_VALUE_INPUT =
  "w-[72px] bg-[rgba(0,0,0,0.3)] border border-[rgba(48,54,61,0.6)] rounded-[4px] px-[6px] py-[3px] text-foreground text-[12px] text-right focus:outline-none focus:border-accent";
const RA_SAVE_BTN =
  "bg-[rgba(56,139,253,0.15)] border border-[rgba(56,139,253,0.4)] text-accent text-[11px] font-semibold px-[12px] py-[4px] rounded-[4px] cursor-pointer transition-[background] duration-100 hover:bg-[rgba(56,139,253,0.28)]";
const RA_CANCEL_EDIT =
  "bg-transparent border border-[rgba(48,54,61,0.6)] rounded-[4px] text-muted text-[11px] px-[8px] py-[4px] cursor-pointer transition-[border-color] duration-100 hover:border-danger hover:text-danger";

const RA_SAVED_SECTION =
  "border-t border-t-[rgba(48,54,61,0.5)] pt-[12px] shrink-0";
const RA_SAVED_GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-[8px]";
const RA_SAVED_CARD =
  "bg-[rgba(0,0,0,0.25)] border border-[rgba(48,54,61,0.5)] rounded-[7px] px-[10px] py-[9px] flex flex-col gap-[3px] transition-[border-color] duration-100 hover:border-[rgba(48,54,61,0.9)]";
const RA_SAVED_CARD_SEL =
  "bg-[rgba(0,0,0,0.25)] border border-[rgba(48,54,61,0.5)] rounded-[7px] px-[10px] py-[9px] flex flex-col gap-[3px] transition-[border-color] duration-100 hover:border-[rgba(48,54,61,0.9)] border-accent! bg-[rgba(56,139,253,0.06)]!";
const RA_SAVED_HEADER = "flex items-center gap-[4px] mb-[3px]";
const RA_LABEL_INPUT =
  "flex-1 bg-transparent border-0 border-b border-b-transparent text-foreground text-[11px] font-semibold px-[2px] py-0 min-w-0 focus:outline-none focus:border-b-accent";
const RA_SAVED_ACTIONS = "flex gap-[3px] shrink-0";
const RA_CMP_BASE =
  "bg-transparent border border-[rgba(48,54,61,0.6)] rounded-[3px] text-muted text-[10px] w-[20px] h-[20px] cursor-pointer flex items-center justify-center transition-all duration-100 hover:border-accent hover:text-accent";
const RA_CMP_ACTIVE =
  "bg-transparent border border-[rgba(48,54,61,0.6)] rounded-[3px] text-muted text-[10px] w-[20px] h-[20px] cursor-pointer flex items-center justify-center transition-all duration-100 hover:border-accent hover:text-accent bg-[rgba(56,139,253,0.2)]! border-accent! text-accent!";
const RA_DELETE_BTN =
  "bg-transparent border border-[rgba(48,54,61,0.6)] rounded-[3px] text-muted text-[10px] w-[20px] h-[20px] cursor-pointer transition-all duration-100 hover:border-danger hover:text-danger";
const RA_EDIT_BTN =
  "bg-transparent border border-[rgba(48,54,61,0.6)] rounded-[3px] text-muted text-[11px] w-[20px] h-[20px] cursor-pointer transition-all duration-100 hover:border-accent hover:text-accent";
const RA_SAVED_STATS = "flex flex-col gap-[2px]";
const RA_SAVED_STAT = "text-[11px] text-muted flex gap-[4px]";

const RA_COMPARE_PANEL =
  "mt-[12px] bg-[rgba(0,0,0,0.2)] border border-[rgba(56,139,253,0.3)] rounded-[7px] px-[12px] py-[10px]";
const RA_COMPARE_GRID = "grid grid-cols-2 gap-[12px]";
const RA_COMPARE_COL = "flex flex-col gap-[3px]";
const RA_COMPARE_LABEL =
  "text-[11px] font-bold text-foreground mb-[4px] pb-[4px] border-b border-b-[rgba(48,54,61,0.4)]";

const RA_SIGN_BASE =
  "min-w-[22px] h-[22px] rounded-[4px] border text-[13px] font-bold cursor-pointer shrink-0 transition-all duration-100";
const RA_SIGN_POS =
  RA_SIGN_BASE +
  " bg-[rgba(63,185,80,0.15)] border-[rgba(63,185,80,0.5)] text-[#3fb950] hover:bg-[rgba(63,185,80,0.3)]";
const RA_SIGN_NEG =
  RA_SIGN_BASE +
  " bg-[rgba(248,81,73,0.12)] border-[rgba(248,81,73,0.5)] text-[#f85149] hover:bg-[rgba(248,81,73,0.25)]";
const RA_FMT_BTN =
  "min-w-[24px] h-[22px] bg-[rgba(255,255,255,0.06)] border border-[rgba(48,54,61,0.7)] rounded-[4px] text-muted text-[11px] font-semibold cursor-pointer shrink-0 transition-[background] duration-100 hover:bg-[rgba(255,255,255,0.12)]";

const RA_ALTERNATIVES = "flex flex-col gap-[8px]";
const RA_ALT_CARD =
  "bg-[rgba(0,0,0,0.2)] border border-[rgba(48,54,61,0.5)] rounded-[7px] px-[12px] py-[10px] flex flex-col gap-[6px]";
const RA_ALT_HEADER = "flex items-center gap-[8px] flex-wrap";
const RA_ALT_LABEL =
  "text-[10px] font-bold uppercase tracking-[0.05em] text-muted bg-[rgba(255,255,255,0.06)] rounded-[3px] px-[6px] py-[2px]";

// ── Types ─────────────────────────────────────────────────────────────────────

function verdictColor2(v: string) {
  if (v.startsWith("GREAT")) return "var(--green)";
  if (v.startsWith("GOOD"))  return "#a8d8a8";
  if (v.startsWith("MED"))   return "#f0c040";
  return "var(--red)";
}

// All riven stats in one list — sign (+/-) is set per-roll by the user
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


// ── Verdict colour helper ─────────────────────────────────────────────────────

function verdictColor(verdict: string): string {
  if (verdict.startsWith("GREAT"))    return "var(--green)";
  if (verdict.startsWith("GOOD"))     return "#a8d8a8";
  if (verdict.startsWith("MEDIOCRE")) return "#f0c040";
  return "var(--red)";
}

// ── Stat score bar ────────────────────────────────────────────────────────────

function ScoreBar({ score }: { score: number }) {
  const pct = Math.round(score * 100);
  const color = score >= 0.8 ? "var(--green)" : score >= 0.6 ? "#a8d8a8" : score >= 0.4 ? "#f0c040" : "var(--red)";
  return (
    <div className="flex items-center gap-[8px] h-[6px] bg-[rgba(255,255,255,0.08)] rounded-[3px] overflow-visible relative">
      <div
        className="h-[6px] rounded-[3px] transition-[width,background] duration-300 min-w-[4px]"
        style={{ width: `${pct}%`, background: color }}
      />
      <span
        className="text-[11px] font-bold absolute right-0 top-[-2px] tabular-nums"
        style={{ color }}
      >
        {pct}%
      </span>
    </div>
  );
}

// ── Main component ────────────────────────────────────────────────────────────

export default function RivenAnalyzer() {
  const [weapons, setWeapons]         = useState<string[]>([]);
  const [weaponInput, setWeaponInput] = useState("");
  const [filtered, setFiltered]       = useState<string[]>([]);
  const [selectedWeapon, setSelectedWeapon] = useState("");
  const [analysis, setAnalysis]       = useState<RivenAnalysis | null>(null);
  const [_rollCount, setRollCount]     = useState(0);

  // Unified stat builder: each stat has a name, value, sign, and format
  const [builtStats, setBuiltStats]   = useState<RivenStat[]>([]);
  // editingId: if set, the save button becomes "Update" and targets this saved roll
  const [editingId, setEditingId]     = useState<string | null>(null);

  // Inline card edit state
  const [inlineEditId, setInlineEditId]       = useState<string | null>(null);
  const [inlineEditLabel, setInlineEditLabel] = useState("");
  const [inlineEditStats, setInlineEditStats] = useState<RivenStat[]>([]);

  // Derive positives/negatives for the analysis call
  const positives = builtStats.filter(s => s.positive).map(s => s.name);
  const negative  = builtStats.find(s => !s.positive)?.name ?? "";

  const toggleStat = (name: string) => {
    setBuiltStats(prev => {
      const exists = prev.find(s => s.name === name);
      if (exists) return prev.filter(s => s.name !== name);
      // Damage-to stats default to × multiplier format
      const useMultiplier = name.startsWith("Damage to");
      return [...prev, { name, value: "", positive: true, useMultiplier }];
    });
  };

  const updateStatValue = (name: string, value: string) =>
    setBuiltStats(prev => prev.map(s => s.name === name ? { ...s, value } : s));

  const toggleStatSign = (name: string) =>
    setBuiltStats(prev => prev.map(s => s.name === name ? { ...s, positive: !s.positive } : s));

  const toggleStatFormat = (name: string) =>
    setBuiltStats(prev => prev.map(s =>
      s.name === name ? { ...s, useMultiplier: !s.useMultiplier } : s
    ));

  const [dbStatus, setDbStatus]       = useState("");
  const [showLog, setShowLog]         = useState(false);
  const [sessionLog, setSessionLog]   = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // ── Saved rivens ────────────────────────────────────────────────────────────
  const [savedRivens, setSavedRivens] = useState<SavedRiven[]>([]);
  const [saveStatus, setSaveStatus]   = useState("");
  // Comparison: set of selected ids
  const [compareIds, setCompareIds]   = useState<Set<string>>(new Set());
  // Inline rename: id → draft label
  const [renameDraft, setRenameDraft] = useState<Record<string, string>>({});

  const loadSavedRivens = useCallback(async () => {
    const list = await invoke<SavedRiven[]>("get_saved_riven_rolls").catch(() => []);
    setSavedRivens(list);
  }, []);

  useEffect(() => { loadSavedRivens(); }, [loadSavedRivens]);

  const saveCurrentRoll = async () => {
    if (!selectedWeapon) return;
    const stats = builtStats.filter(s => s.value.trim() !== "");
    if (stats.length === 0) { setSaveStatus("Add stat values before saving."); return; }
    const now = new Date();
    try {
      if (editingId) {
        // Update existing roll
        await invoke("delete_saved_riven_roll", { id: editingId });
        const args: SaveRivenRollArgs = {
          weapon: selectedWeapon,
          label: savedRivens.find(r => r.id === editingId)?.label ?? `${selectedWeapon.charAt(0).toUpperCase() + selectedWeapon.slice(1)} · ${now.getDate()} ${now.toLocaleString("en",{month:"short"})}`,
          statsJson: JSON.stringify(stats),
          verdict: analysis?.verdict ?? "", score: analysis?.score ?? 0,
        };
        await invoke(TAURI_COMMANDS.SAVE_RIVEN_ROLL, args);
        setEditingId(null);
        setSaveStatus("Updated!");
      } else {
        const label = `${selectedWeapon.charAt(0).toUpperCase() + selectedWeapon.slice(1)} · ${now.getDate()} ${now.toLocaleString("en",{month:"short"})} ${now.getFullYear()}`;
        const args: SaveRivenRollArgs = {
          weapon: selectedWeapon, label, statsJson: JSON.stringify(stats),
          verdict: analysis?.verdict ?? "", score: analysis?.score ?? 0,
        };
        await invoke(TAURI_COMMANDS.SAVE_RIVEN_ROLL, args);
        setSaveStatus("Saved!");
      }
      loadSavedRivens();
      setTimeout(() => setSaveStatus(""), 2000);
    } catch (e: unknown) { setSaveStatus(String(e)); }
  };

  const startInlineEdit = (r: SavedRiven) => {
    const stats: RivenStat[] = (() => { try { return JSON.parse(r.stats_json); } catch { return []; } })();
    setInlineEditId(r.id);
    setInlineEditLabel(r.label);
    setInlineEditStats(stats);
  };

  const saveInlineEdit = async (r: SavedRiven) => {
    const stats = inlineEditStats.filter(s => s.value.trim() !== "");
    try {
      await invoke("delete_saved_riven_roll", { id: r.id });
      const args: SaveRivenRollArgs = {
        weapon: r.weapon,
        label: inlineEditLabel,
        statsJson: JSON.stringify(stats),
        verdict: r.verdict,
        score: r.score,
      };
      await invoke(TAURI_COMMANDS.SAVE_RIVEN_ROLL, args);
      loadSavedRivens();
      setInlineEditId(null);
    } catch {}
  };

  const deleteSaved = async (id: string) => {
    await invoke("delete_saved_riven_roll", { id }).catch(() => {});
    setSavedRivens(prev => prev.filter(r => r.id !== id));
    setCompareIds(prev => { const s = new Set(prev); s.delete(id); return s; });
  };

  const applyRename = async (id: string) => {
    const label = renameDraft[id]?.trim();
    if (!label) return;
    await invoke("rename_saved_riven_roll", { id, label }).catch(() => {});
    setSavedRivens(prev => prev.map(r => r.id === id ? { ...r, label } : r));
    setRenameDraft(prev => { const d = { ...prev }; delete d[id]; return d; });
  };

  const toggleCompare = (id: string) => {
    setCompareIds(prev => {
      const s = new Set(prev);
      if (s.has(id)) { s.delete(id); } else if (s.size < 2) { s.add(id); }
      return s;
    });
  };

  const compareList = savedRivens.filter(r => compareIds.has(r.id));

  // Load weapons list on mount
  useEffect(() => {
    invoke<string[]>("get_riven_weapons")
      .then(w => { setWeapons(w); setDbStatus(`${w.length} weapons loaded`); })
      .catch(() => setDbStatus("Failed to load database — click Refresh"));
  }, []);

  // Filter weapon suggestions
  useEffect(() => {
    if (!weaponInput.trim()) { setFiltered([]); return; }
    const q = weaponInput.toLowerCase();
    setFiltered(weapons.filter(w => w.includes(q)).slice(0, 8));
  }, [weaponInput, weapons]);

  // Listen for EE.log riven events
  useEffect(() => {
    const unlistenUnveil  = listen(TAURI_EVENTS.RIVEN_UNVEILED, () => {
      setRollCount(0);
      inputRef.current?.focus();
    });
    const unlistenSaved = listen(TAURI_EVENTS.RIVEN_ROLL_SAVED, () => loadSavedRivens());
    return () => {
      unlistenUnveil.then(fn => fn());
      unlistenSaved.then(fn => fn());
    };
  }, [selectedWeapon, positives, negative]); // eslint-disable-line

  const selectWeapon = (w: string) => {
    setSelectedWeapon(w);
    setWeaponInput(w.charAt(0).toUpperCase() + w.slice(1));
    setFiltered([]);
    setBuiltStats([]);
    setAnalysis(null);
    setRollCount(0);
    setEditingId(null);
  };

  const runAnalysis = useCallback(async () => {
    if (!selectedWeapon || builtStats.length === 0) { setAnalysis(null); return; }
    const args: AnalyzeRivenArgs = {
      weapon: selectedWeapon,
      positives: builtStats.filter(s => s.positive).map(s => s.name),
      negatives: builtStats.filter(s => !s.positive).map(s => s.name),
    };
    const result = await invoke<RivenAnalysis | null>(TAURI_COMMANDS.ANALYZE_RIVEN, { ...args });
    setAnalysis(result ?? null);
  }, [selectedWeapon, builtStats]);

  useEffect(() => { if (selectedWeapon) runAnalysis(); }, [builtStats, runAnalysis]);

  const reset = () => {
    setBuiltStats([]);
    setAnalysis(null);
    setEditingId(null);
    setRollCount(c => c + 1);
  };

  const reloadDb = async () => {
    setDbStatus("Reloading…");
    try {
      const count = await invoke<number>("reload_riven_database");
      const w = await invoke<string[]>("get_riven_weapons");
      setWeapons(w);
      setDbStatus(`${count} weapons loaded`);
    } catch { setDbStatus("Reload failed"); }
  };

  return (
    <div className={RA_ANALYZER}>
      {/* Header */}
      <div className={RA_HEADER}>
        <span className={RA_TITLE}>Riven Analyzer</span>
        <button
          className={RA_CHECK_BTN}
          onClick={() => checkRivenNow()}
          title="Capture current riven card from Warframe screen"
        >
          🔍 Check Riven
        </button>
        <span className={RA_DB_STATUS}>{dbStatus}</span>
        <button
          className={RA_CREDIT}
          title="Open Riven price database on Google Sheets"
          onClick={() => invoke(TAURI_COMMANDS.OPEN_URL, { url: "https://docs.google.com/spreadsheets/d/1zbaeJBuBn44cbVKzJins_E3hTDpnmvOk8heYN-G8yy8" }).catch(() => {})}
        >data by 44bananas ↗</button>
        <button className={RA_REFRESH_BTN} onClick={reloadDb} title="Reload database from Google Sheet">↻</button>
        <button className={RA_REFRESH_BTN} title="View session log" onClick={async () => {
          const log = await invoke<string>("get_riven_session_log").catch(() => "Log unavailable");
          setSessionLog(log);
          setShowLog(v => !v);
        }}>📋</button>
      </div>

      {/* Weapon search */}
      <div className={RA_WEAPON_WRAP}>
        <input
          ref={inputRef}
          className={RA_WEAPON_INPUT}
          placeholder="Type weapon name…"
          value={weaponInput}
          onChange={e => { setWeaponInput(e.target.value); setSelectedWeapon(""); setAnalysis(null); }}
        />
        {filtered.length > 0 && (
          <div className={RA_SUGGESTIONS}>
            {filtered.map(w => (
              <div key={w} className={RA_SUGGESTION} onClick={() => selectWeapon(w)}>
                {w.charAt(0).toUpperCase() + w.slice(1)}
              </div>
            ))}
          </div>
        )}
      </div>

      {showLog && (
        <pre className="max-h-[300px] shrink-0 overflow-y-auto whitespace-pre-wrap break-all rounded-[5px] border border-[rgba(48,54,61,.6)] bg-black/30 p-2.5 text-[10px] text-muted">
          {sessionLog}
        </pre>
      )}

      {selectedWeapon && (
        <>
          {/* Unified stat picker — click to add, sign toggled below */}
          <div className={RA_SECTION_LABEL}>Select stats rolled <span className={RA_OPTIONAL}>(click to add, set + / − below)</span></div>
          <div className={RA_STAT_GRID}>
            {ALL_STATS.map(stat => {
              const entry = builtStats.find(s => s.name === stat);
              return (
                <button
                  key={stat}
                  className={entry?.positive ? RA_STAT_BTN_SELECTED : RA_STAT_BTN}
                  onClick={() => toggleStat(stat)}
                >
                  {entry ? (entry.positive ? "+" : "−") : ""}{stat}
                </button>
              );
            })}
          </div>

          {/* Per-stat value rows with +/- and %/× toggles */}
          {builtStats.length > 0 && (
            <div className={RA_VALUE_INPUTS}>
              <div className={RA_SECTION_LABEL}>Stat values</div>
              {builtStats.map(stat => (
                <div key={stat.name} className={RA_VALUE_ROW}>
                  {/* +/- toggle */}
                  <button
                    className={stat.positive ? RA_SIGN_POS : RA_SIGN_NEG}
                    onClick={() => toggleStatSign(stat.name)}
                    title="Toggle positive / negative"
                  >{stat.positive ? "+" : "−"}</button>
                  <span className={RA_VALUE_LABEL}>{stat.name}</span>
                  <input
                    className={RA_VALUE_INPUT}
                    placeholder={stat.useMultiplier ? "e.g. 0.88" : "e.g. 85"}
                    value={stat.value}
                    onChange={e => updateStatValue(stat.name, e.target.value)}
                  />
                  {/* %/× toggle — click to switch format */}
                  <button
                    className={RA_FMT_BTN}
                    onClick={() => toggleStatFormat(stat.name)}
                    title="Click to switch between % and × (multiplier)"
                  >
                    {stat.useMultiplier ? "×" : "%"}
                  </button>
                </div>
              ))}
              <div className="mt-1 flex items-center gap-2">
                <button className={RA_SAVE_BTN} onClick={saveCurrentRoll}>
                  {editingId ? "✓ Update Roll" : "💾 Save Roll"}
                </button>
                {editingId && <button className={RA_CANCEL_EDIT} onClick={reset}>Cancel</button>}
                {saveStatus && <span className={`text-[11px] ${saveStatus.includes("!") || saveStatus.includes("✓") ? "text-green" : "text-red"}`}>{saveStatus}</span>}
              </div>
            </div>
          )}

          {/* Analysis — one card per build alternative */}
          {analysis && (
            <div className={RA_ALTERNATIVES}>
              {analysis.alternatives.map((alt, i) => (
                <div key={i} className={RA_ALT_CARD}>
                  <div className={RA_ALT_HEADER}>
                    {analysis.alternatives.length > 1 && (
                      <span className={RA_ALT_LABEL}>{alt.label}</span>
                    )}
                    <span className={RA_VERDICT} style={{ color: verdictColor(alt.verdict) }}>
                      {alt.verdict}
                    </span>
                  </div>
                  <ScoreBar score={alt.score} />
                  <div className={RA_STATS_BREAKDOWN}>
                    {alt.matched.map(s => (
                      <div key={s} className={STAT_TONE.good.row}>
                        <span className={RA_STAT_ICON}>✓</span><span>{s}</span>
                        <span className={STAT_TONE.good.tag}>Wanted</span>
                      </div>
                    ))}
                    {alt.missing.map(s => (
                      <div key={s} className={STAT_TONE.miss.row}>
                        <span className={RA_STAT_ICON}>○</span><span>{s}</span>
                        <span className={STAT_TONE.miss.tag}>Not rolled</span>
                      </div>
                    ))}
                    {i === 0 && analysis.safe_negatives_present.map(s => (
                      <div key={s} className={STAT_TONE.safe.row}>
                        <span className={RA_STAT_ICON}>✓</span><span>−{s}</span>
                        <span className={STAT_TONE.safe.tag}>Safe neg</span>
                      </div>
                    ))}
                    {i === 0 && analysis.harmful_negatives.map(s => (
                      <div key={s} className={STAT_TONE.bad.row}>
                        <span className={RA_STAT_ICON}>✗</span><span>−{s}</span>
                        <span className={STAT_TONE.bad.tag}>Harmful</span>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
              {analysis.notes && (
                <div className={RA_NOTES}>ℹ {analysis.notes}</div>
              )}
            </div>
          )}

          <button className={RA_NEXT_ROLL} onClick={reset}>
            Next roll →
          </button>
        </>
      )}

      {/* ── Saved Rolls ──────────────────────────────────────────────────────── */}
      {savedRivens.length > 0 && (
        <div className={RA_SAVED_SECTION}>
          <div className={RA_SECTION_LABEL + " mb-2"}>
            Saved Rolls ({savedRivens.length}/50)
            {compareIds.size > 0 && <span className="ml-2 text-[11px] text-accent">
              {compareIds.size === 1 ? "Select 1 more to compare" : "Comparing ↓"}
            </span>}
          </div>

          <div className={RA_SAVED_GRID}>
            {savedRivens.map(r => {
              const stats: RivenStat[] = (() => { try { return JSON.parse(r.stats_json); } catch { return []; } })();
              const isSelected = compareIds.has(r.id);
              const isEditing = inlineEditId === r.id;
              return (
                <div key={r.id} className={isSelected ? RA_SAVED_CARD_SEL : RA_SAVED_CARD}>
                  {/* Card header */}
                  <div className={RA_SAVED_HEADER}>
                    <input
                      className={RA_LABEL_INPUT}
                      value={isEditing ? inlineEditLabel : r.label}
                      onChange={e => isEditing ? setInlineEditLabel(e.target.value) : setRenameDraft(p => ({ ...p, [r.id]: e.target.value }))}
                      onBlur={() => !isEditing && applyRename(r.id)}
                      onKeyDown={e => !isEditing && e.key === "Enter" && applyRename(r.id)}
                    />
                    <div className={RA_SAVED_ACTIONS}>
                      {isEditing ? (<>
                        <button className={RA_CMP_ACTIVE} onClick={() => saveInlineEdit(r)} title="Save changes">✓</button>
                        <button className={RA_DELETE_BTN} onClick={() => setInlineEditId(null)} title="Cancel edit">✕</button>
                      </>) : (<>
                        <button
                          className={isSelected ? RA_CMP_ACTIVE : RA_CMP_BASE}
                          onClick={() => toggleCompare(r.id)}
                          title="Select for comparison"
                        >{isSelected ? "✓" : "⚖"}</button>
                        <button className={RA_EDIT_BTN} onClick={() => startInlineEdit(r)} title="Edit roll">✎</button>
                        <button className={RA_DELETE_BTN} onClick={() => deleteSaved(r.id)} title="Delete">✕</button>
                      </>)}
                    </div>
                  </div>

                  {/* Verdict */}
                  {r.verdict && (
                    <div className="mb-1 text-[11px] font-bold" style={{ color: verdictColor2(r.verdict) }}>
                      {r.verdict.split("—")[0].trim()} · {Math.round(r.score * 100)}%
                    </div>
                  )}

                  {/* Stats — editable in edit mode */}
                  <div className={RA_SAVED_STATS}>
                    {(isEditing ? inlineEditStats : stats).map((s, i) => (
                      <div key={i} className={RA_SAVED_STAT + " items-center gap-1"}>
                        {isEditing ? (<>
                          <button
                            className={(s.positive ? RA_SIGN_POS : RA_SIGN_NEG) + " !size-[18px] !min-w-0 !p-0 !text-[11px]"}
                            onClick={() => setInlineEditStats(prev => prev.map((x, j) => j === i ? { ...x, positive: !x.positive } : x))}
                          >{s.positive ? "+" : "−"}</button>
                          <input
                            className="w-12 rounded-[3px] border border-[rgba(48,54,61,.6)] bg-black/30 px-1 py-px text-right text-[11px] text-foreground"
                            value={s.value}
                            onChange={e => setInlineEditStats(prev => prev.map((x, j) => j === i ? { ...x, value: e.target.value } : x))}
                          />
                          <span className="text-[11px] text-muted">% {s.name}</span>
                        </>) : (<>
                          <span className={s.positive ? "text-[rgba(139,148,158,.7)]" : "text-red"}>
                            {s.positive ? "+" : "−"}
                          </span>
                          <span>{s.value && `${s.value}% `}{s.name}</span>
                        </>)}
                      </div>
                    ))}
                  </div>

                  <div className="mt-1 text-[10px] text-[rgba(139,148,158,.4)]">
                    {r.saved_at.slice(0, 10)}
                  </div>
                </div>
              );
            })}
          </div>

          {/* Comparison panel */}
          {compareList.length === 2 && (
            <div className={RA_COMPARE_PANEL}>
              <div className={RA_SECTION_LABEL + " mb-2"}>Comparison</div>
              <div className={RA_COMPARE_GRID}>
                {compareList.map(r => {
                  const stats: RivenStat[] = (() => { try { return JSON.parse(r.stats_json); } catch { return []; } })();
                  return (
                    <div key={r.id} className={RA_COMPARE_COL}>
                      <div className={RA_COMPARE_LABEL}>{r.label}</div>
                      {r.verdict && (
                        <div className="mb-1.5 text-[11px] font-bold" style={{ color: verdictColor2(r.verdict) }}>
                          {r.verdict.split("—")[0].trim()} · {Math.round(r.score * 100)}%
                        </div>
                      )}
                      {stats.map((s, i) => (
                        <div key={i} className={RA_SAVED_STAT}>
                          <span className={s.positive ? "text-[rgba(139,148,158,.7)]" : "text-red"}>
                            {s.positive ? "+" : "−"}
                          </span>
                          <span>{s.value && `${s.value}% `}{s.name}</span>
                        </div>
                      ))}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
