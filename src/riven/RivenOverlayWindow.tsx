import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";
import type { RivenAnalysis, RivenAnalysisUpdate, RivenStat } from "../types/rivens";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import type { SaveRivenRollArgs } from "../types/tauri";

// Tells App.tsx to run OCR again (for "Check New Roll" / "Start Comparison")
const triggerNewCheck = () => emit(TAURI_EVENTS.RIVEN_MANUAL_CHECK, {}).catch(() => {});

// Save current roll directly from overlay
async function saveOverlayRoll(
  weapon: string,
  stats: RivenStat[],
  verdict: string, score: number, rollCount: number
) {
  if (!weapon || stats.length === 0) return;
  const now = new Date();
  const label = `${weapon.charAt(0).toUpperCase() + weapon.slice(1)} · Roll #${rollCount} · ${now.getDate()} ${now.toLocaleString("en",{month:"short"})}`;
  const args: SaveRivenRollArgs = {
    weapon, label, statsJson: JSON.stringify(stats), verdict, score,
  };
  await invoke(TAURI_COMMANDS.SAVE_RIVEN_ROLL, args).catch(() => {});
  await emit(TAURI_EVENTS.RIVEN_ROLL_SAVED).catch(() => {});
}

import "./RivenOverlayWindow.css";

// ── Tailwind class constants (formerly RivenOverlayWindow.css) ────────────────

const ROV_ROOT = "w-full h-full flex items-start justify-center p-2";
const ROV_CARD =
  "bg-[rgba(13,17,23,0.93)] border border-[rgba(56,139,253,0.3)] rounded-[10px] px-[14px] py-[12px] w-full flex flex-col gap-[8px] backdrop-blur-[10px] shadow-[0_8px_32px_rgba(0,0,0,0.65)]";

const ROV_HEADER = "flex items-center gap-[6px]";
const ROV_TITLE = "text-[14px] font-bold text-[#e6edf3] flex-1";
const ROV_COMPARE =
  "bg-[rgba(56,139,253,0.15)] border border-[rgba(56,139,253,0.35)] text-[#58a6ff] text-[10px] font-semibold cursor-pointer px-[7px] py-[2px] rounded-[4px] leading-[1.4] transition-[background] duration-150 hover:bg-[rgba(56,139,253,0.28)]";
const ROV_SAVE =
  "bg-transparent border-0 text-[rgba(139,148,158,0.6)] text-[12px] cursor-pointer px-[4px] py-[2px] rounded-[4px] leading-none transition-colors duration-150 hover:text-[#3fb950]";
const ROV_CLOSE =
  "bg-transparent border-0 text-[rgba(139,148,158,0.5)] text-[12px] cursor-pointer px-[4px] py-[2px] rounded-[4px] leading-none transition-[color,background] duration-150 hover:text-[#f85149] hover:bg-[rgba(248,81,73,0.12)]";

const ROV_SCANNING = "text-[12px] text-[rgba(139,148,158,0.8)] text-center py-[6px]";
const ROV_VERDICT = "text-[13px] font-bold tracking-[0.3px]";

const ROV_SCORE_WRAP = "flex items-center gap-[7px]";
const ROV_SCORE_TRACK =
  "flex-1 h-[5px] bg-[rgba(255,255,255,0.08)] rounded-[3px] overflow-hidden";
const ROV_SCORE_FILL = "h-full rounded-[3px] transition-[width] duration-[400ms] ease-[ease]";
const ROV_SCORE_PCT = "text-[11px] font-semibold min-w-[30px] text-right";

const ROV_ROLLED =
  "flex flex-col gap-[4px] border-t border-t-[rgba(255,255,255,0.06)] pt-[8px]";
const ROV_ROW = "flex items-center gap-[7px] text-[12px] px-[6px] py-[3px] rounded-[5px]";
const ROV_ICON = "text-[11px] w-[13px] text-center shrink-0";
const ROV_NAME = "flex-1 text-[#e6edf3]";
const ROV_VALUE = "text-[12px] font-semibold tabular-nums shrink-0";
const STAT_TONE: Record<string, { row: string; icon: string; name: string; value: string }> = {
  wanted: {
    row: ROV_ROW + " bg-[rgba(63,185,80,0.1)]",
    icon: ROV_ICON + " text-[#3fb950]",
    name: ROV_NAME,
    value: ROV_VALUE + " text-[#3fb950]",
  },
  neutral: {
    row: ROV_ROW + " bg-[rgba(255,255,255,0.04)]",
    icon: ROV_ICON + " text-[rgba(139,148,158,0.6)]",
    name: "flex-1 text-[rgba(230,237,243,0.7)]",
    value: ROV_VALUE + " text-[#f0c040]",
  },
  safe_neg: {
    row: ROV_ROW + " bg-[rgba(56,139,253,0.08)]",
    icon: ROV_ICON + " text-[#58a6ff]",
    name: ROV_NAME,
    value: ROV_VALUE + " text-[#58a6ff]",
  },
  harmful: {
    row: ROV_ROW + " bg-[rgba(248,81,73,0.08)]",
    icon: ROV_ICON + " text-[#f85149]",
    name: ROV_NAME,
    value: ROV_VALUE + " text-[#f85149]",
  },
};

const ROV_ORIGINAL = "border-t border-t-[rgba(255,255,255,0.06)] pt-[6px]";
const ROV_SECTION_LABEL =
  "text-[10px] font-semibold text-[rgba(139,148,158,0.55)] uppercase tracking-[0.5px] mb-[4px]";
const ROV_ALT_CARD =
  "border-t border-t-[rgba(255,255,255,0.06)] pt-[6px] first:border-t-0 first:pt-0";
const ROV_ALT_LABEL =
  "text-[9px] font-bold uppercase tracking-[0.05em] text-[rgba(139,148,158,0.55)] bg-[rgba(255,255,255,0.06)] rounded-[3px] px-[5px] py-[1px] inline-block mb-[3px]";
const ROV_MISSING =
  "text-[10.5px] text-[rgba(139,148,158,0.65)] border-t border-t-[rgba(255,255,255,0.06)] pt-[6px] leading-[1.6] break-words";
const ROV_MISSING_LABEL = "font-semibold";
const ROV_MISSING_STAT = "text-[rgba(139,148,158,0.9)]";
const ROV_NOTES =
  "text-[10px] text-[rgba(139,148,158,0.7)] italic border-t border-t-[rgba(255,255,255,0.06)] pt-[5px]";

// No auto-hide — user dismisses with ✕ or the poll detects screen closure.
// Only a very long emergency fallback (60 min) in case everything else fails.

// Ask App.tsx to hide this overlay — App.tsx owns the rivenWin reference
// Tell App.tsx to hide the overlay AND log the reason BEFORE hiding
const requestHide = (reason: string) => {
  emit(TAURI_EVENTS.RIVEN_OVERLAY_HIDE, { reason }).catch(() => {});
};

function verdictColor(verdict: string): string {
  if (verdict.startsWith("GREAT"))    return "#3fb950";
  if (verdict.startsWith("GOOD"))     return "#a8d8a8";
  if (verdict.startsWith("MEDIOCRE")) return "#f0c040";
  return "#f85149";
}

function ScoreBar({ score }: { score: number }) {
  const pct = Math.round(score * 100);
  const color = score >= 0.8 ? "#3fb950" : score >= 0.6 ? "#a8d8a8" : score >= 0.4 ? "#f0c040" : "#f85149";
  return (
    <div className={ROV_SCORE_WRAP}>
      <div className={ROV_SCORE_TRACK}>
        <div className={ROV_SCORE_FILL} style={{ width: `${pct}%`, background: color }} />
      </div>
      <span className={ROV_SCORE_PCT} style={{ color }}>{pct}%</span>
    </div>
  );
}

export default function RivenOverlayWindow() {
  const [analysis, setAnalysis]         = useState<RivenAnalysis | null>(null);
  const [rolledStats, setRolledStats]   = useState<RivenStat[]>([]);
  const [originalStats, setOriginalStats] = useState<RivenStat[]>([]);
  const [isComparison, setIsComparison] = useState(false);
  const [ocrRaw, setOcrRaw]             = useState("");
  const [parsedWeapon, setParsedWeapon] = useState("");
  const [rollCount, setRollCount]       = useState(0);
  const [scanning, setScanning]         = useState(true);
  const [saved, setSaved]               = useState(false);
  const scanTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const resetToScanning = () => {
    setScanning(true);
    setAnalysis(null);
    setRolledStats([]);
    setOriginalStats([]);
    setIsComparison(false);
    setOcrRaw("");
    setParsedWeapon("");
    if (scanTimerRef.current) clearTimeout(scanTimerRef.current);
    // 60-min emergency fallback — only fires if poll completely breaks
    scanTimerRef.current = setTimeout(() => requestHide("emergency-60min"), 3_600_000);
  };

  useEffect(() => {
    const unlistenStart = listen(TAURI_EVENTS.RIVEN_SCANNING_START, () => resetToScanning());

    const unlistenUpdate = listen<RivenAnalysisUpdate>(TAURI_EVENTS.RIVEN_ANALYSIS_UPDATE, e => {
      if (scanTimerRef.current) clearTimeout(scanTimerRef.current);
      setAnalysis(e.payload.analysis ?? null);
      setRollCount(e.payload.rollCount);
      setOcrRaw(e.payload.ocrRaw ?? "");
      setParsedWeapon(e.payload.weapon ?? "");
      setRolledStats(e.payload.rolledStats ?? []);
      setOriginalStats(e.payload.originalStats ?? []);
      setIsComparison(e.payload.isComparison ?? false);
      setScanning(false);
      // Reset emergency fallback timer — 60 min from last data shown
      scanTimerRef.current = setTimeout(() => requestHide("emergency-60min"), 3_600_000);
    });

    // Tell App.tsx the listener is registered and the pending payload can be sent now.
    emit(TAURI_EVENTS.RIVEN_WINDOW_READY, {}).catch(() => {});

    // Initial hide fallback — same as resetToScanning's timer
    scanTimerRef.current = setTimeout(() => requestHide("emergency-60min"), 3_600_000);

    return () => {
      unlistenStart.then(fn => fn());
      unlistenUpdate.then(fn => fn());
      if (scanTimerRef.current) clearTimeout(scanTimerRef.current);
    };
  }, []); // eslint-disable-line

  const weaponName = analysis?.weapon ?? parsedWeapon;
  const displayName = weaponName
    ? weaponName.split(" ").map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(" ")
    : "Riven Analyzer";

  // All wanted stat names — matched (in new roll) + missing (from groups like "MS / TOX / DMG")
  const allWantedNames = new Set<string>();
  if (analysis) {
    analysis.matched_positives.forEach(s => allWantedNames.add(s));
    analysis.missing_positives.forEach(group =>
      group.split(" / ").forEach(s => allWantedNames.add(s.trim()))
    );
  }

  // Classify a rolled stat against the analysis
  const classifyStat = (stat: RivenStat): "wanted" | "neutral" | "safe_neg" | "harmful" => {
    if (!analysis) return "neutral";
    if (!stat.positive) {
      return analysis.safe_negatives_present.includes(stat.name) ? "safe_neg" : "harmful";
    }
    return analysis.matched_positives.includes(stat.name) ? "wanted" : "neutral";
  };

  // Classify an original-roll stat — same logic but uses the full wanted list
  const classifyOriginalStat = (stat: RivenStat): "wanted" | "neutral" | "safe_neg" | "harmful" => {
    if (!analysis) return "neutral";
    if (!stat.positive) {
      // For original negatives, mark as safe if in the weapon's safe list (same DB)
      return analysis.safe_negatives_present.includes(stat.name) ? "safe_neg" : "neutral";
    }
    return allWantedNames.has(stat.name) ? "wanted" : "neutral";
  };

  const statIcon = (cls: "wanted" | "neutral" | "safe_neg" | "harmful") => {
    if (cls === "wanted")   return "✓";
    if (cls === "safe_neg") return "✓";
    if (cls === "harmful")  return "✗";
    return "○";
  };

  return (
    <div className={ROV_ROOT}>
      <div className={ROV_CARD}>
        {/* Header */}
        <div className={ROV_HEADER}>
          <span className={ROV_TITLE}>{displayName}</span>
          <button className={ROV_COMPARE} onClick={() => triggerNewCheck()} title="Re-scan (use after cycling for comparison)">
            {isComparison ? "🔄 Refresh" : "⚡ New Roll"}
          </button>
          {rolledStats.length > 0 && (
            <button className={ROV_SAVE} title="Save this roll"
              onClick={async () => {
                await saveOverlayRoll(weaponName, rolledStats, analysis?.verdict ?? "", analysis?.score ?? 0, rollCount);
                setSaved(true); setTimeout(() => setSaved(false), 2000);
              }}>
              {saved ? "✓" : "💾"}
            </button>
          )}
          <button className={ROV_CLOSE} onClick={() => requestHide("x-button")} title="Dismiss">✕</button>
        </div>

        {/* Scanning */}
        {scanning && (
          <div className={ROV_SCANNING}>Scanning stats…</div>
        )}

        {/* No result */}
        {!scanning && rolledStats.length === 0 && !analysis && (
          <div className={ROV_SCANNING + " !text-danger"}>
            Could not read card stats
            {parsedWeapon && <div className="mt-[3px] text-[10px] text-[rgba(139,148,158,.7)]">Weapon: "{parsedWeapon}"</div>}
          </div>
        )}

        {/* Result */}
        {!scanning && (rolledStats.length > 0 || analysis) && (
          <>
            {/* One analysis card per build alternative */}
            {analysis && analysis.alternatives.map((alt, i) => (
              <div key={i} className={ROV_ALT_CARD}>
                {analysis.alternatives.length > 1 && (
                  <span className={ROV_ALT_LABEL}>{alt.label}</span>
                )}
                <div className={ROV_VERDICT} style={{ color: verdictColor(alt.verdict) }}>
                  {alt.verdict}
                </div>
                <ScoreBar score={alt.score} />
                {/* Negatives shown once on first card */}
                {i === 0 && analysis.safe_negatives_present.map(s => (
                  <div key={s} className={STAT_TONE.safe_neg.row}>
                    <span className={STAT_TONE.safe_neg.icon}>✓</span>
                    <span className={STAT_TONE.safe_neg.name}>−{s}</span>
                    <span className={STAT_TONE.safe_neg.value + " !text-[10px]"}>Safe</span>
                  </div>
                ))}
                {i === 0 && analysis.harmful_negatives.map(s => (
                  <div key={s} className={STAT_TONE.harmful.row}>
                    <span className={STAT_TONE.harmful.icon}>✗</span>
                    <span className={STAT_TONE.harmful.name}>−{s}</span>
                    <span className={STAT_TONE.harmful.value + " !text-[10px]"}>Harmful</span>
                  </div>
                ))}
                {alt.missing.length > 0 && (
                  <div className={ROV_MISSING}>
                    <span className={ROV_MISSING_LABEL}>Wanted: </span>
                    {alt.missing.map((s, j) => (
                      <span key={s} className={ROV_MISSING_STAT}>
                        {s}{j < alt.missing.length - 1 ? ", " : ""}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            ))}

            {/* Rolled stats — what's actually on the card */}
            {rolledStats.length > 0 && (
              <div className={ROV_ROLLED}>
              {isComparison && <div className={ROV_SECTION_LABEL}>New roll</div>}
                {rolledStats.map((stat, i) => {
                  const cls = classifyStat(stat);
                  const tone = STAT_TONE[cls];
                  return (
                    <div key={i} className={tone.row}>
                      <span className={tone.icon}>{statIcon(cls)}</span>
                      <span className={tone.name}>{stat.name}</span>
                      <span className={tone.value}>{stat.value}</span>
                    </div>
                  );
                })}
              </div>
            )}

            {/* Original roll (comparison mode only) — same quality colors as new roll */}
            {isComparison && originalStats.length > 0 && (
              <div className={ROV_ORIGINAL}>
                <div className={ROV_SECTION_LABEL}>Original roll</div>
                <div className={ROV_ROLLED}>
                  {originalStats.map((stat, i) => {
                    const cls = classifyOriginalStat(stat);
                    const tone = STAT_TONE[cls];
                    return (
                      <div key={i} className={tone.row + " opacity-75"}>
                        <span className={tone.icon}>{statIcon(cls)}</span>
                        <span className={tone.name}>{stat.name}</span>
                        <span className={tone.value}>{stat.value}</span>
                      </div>
                    );
                  })}
                </div>
              </div>
            )}


            {/* No DB entry */}
            {!analysis && rolledStats.length > 0 && (
              <div className={ROV_MISSING + " !text-[rgba(139,148,158,.6)]"}>
                No database entry for {displayName}
              </div>
            )}

            {analysis?.notes && (
              <div className={ROV_NOTES}>ℹ {analysis.notes}</div>
            )}

            {/* Raw OCR fallback */}
            {!analysis && ocrRaw && (
              <details className="mt-1">
                <summary className="cursor-pointer text-[10px] text-[rgba(139,148,158,.5)]">Raw OCR</summary>
                <pre className="mt-[3px] max-h-[100px] overflow-y-auto whitespace-pre-wrap text-[9px] text-[rgba(139,148,158,.6)]">{ocrRaw}</pre>
              </details>
            )}
          </>
        )}
      </div>
    </div>
  );
}
