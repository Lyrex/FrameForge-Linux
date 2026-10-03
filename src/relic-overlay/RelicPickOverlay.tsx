import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { clampToMonitor, overlayScale } from "../lib/uiScale";
import { DEFAULT_RELIC_PICK_LINES, DEFAULT_RELIC_PICK_PRIORITY, RELIC_PICK_LINES_OPTIONS, RELIC_PICK_PRIORITY_OPTIONS } from "../constants/settings";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import type { SettingsFile } from "../types/tauri";
import type { RelicPickPayload, RelicPickRelic, RelicPickReward } from "../types/relics";
import type { RelicPickLines, RelicPickPriority } from "../types/settings";
import "./RelicPickOverlay.css";

// ── Tailwind class constants (formerly RelicPickOverlay.css) ──────────────────
// Note: body transparency stays in RelicPickOverlay.css (document context).

const RPO_ROOT =
  "flex flex-col gap-[4px] py-[6px] px-[8px] bg-[rgba(13,17,23,0.92)] border border-[rgba(56,139,253,0.4)] rounded-[8px] text-[12px] text-[color:var(--text,#e6edf3)] w-full h-auto backdrop-blur-[4px]";

const RPO_HEADER = "flex items-center gap-[6px] shrink-0";
const RPO_TITLE =
  "text-[12px] font-bold text-[#58a6ff] tracking-[0.04em] uppercase flex-1";
const RPO_CLOSE =
  "bg-transparent border-0 text-[rgba(230,237,243,0.35)] text-[13px] cursor-pointer px-[2px] py-0 leading-none transition-colors duration-100 hover:text-[rgba(230,237,243,0.85)]";
const RPO_EMPTY =
  "px-[8px] py-[12px] text-center text-[rgba(230,237,243,0.4)] text-[11px] whitespace-nowrap";
const RPO_LIST = "flex flex-col gap-[4px] overflow-y-auto max-h-[460px]";

const RPO_CARD =
  "bg-[rgba(255,255,255,0.04)] border border-[rgba(48,54,61,0.6)] rounded-[5px] overflow-hidden shrink-0";
const RPO_CARD_BEST =
  "bg-[rgba(56,139,253,0.06)] border border-[rgba(56,139,253,0.5)] rounded-[5px] overflow-hidden shrink-0";
const RPO_CARD_HEADER =
  "flex items-center gap-[5px] px-[8px] py-[4px] border-b border-b-[rgba(48,54,61,0.5)]";
const RPO_RANK = "text-[10px] font-bold text-[rgba(230,237,243,0.3)] min-w-[16px]";
const RPO_RANK_BEST = "text-[10px] font-bold text-[#58a6ff] min-w-[16px]";
const RPO_RELIC_NAME =
  "flex-1 font-semibold text-[12px] whitespace-nowrap overflow-hidden text-ellipsis";

const REF_SHAPE =
  "text-[9px] font-semibold px-[4px] py-[1px] rounded-[3px] whitespace-nowrap";
const REF_BADGE: Record<string, string> = {
  intact:
    REF_SHAPE +
    " bg-[rgba(255,255,255,0.07)] text-[rgba(230,237,243,0.5)] border border-[rgba(255,255,255,0.1)]",
  exceptional:
    REF_SHAPE +
    " text-[#79c0ff] border border-[rgba(121,192,255,0.3)] bg-[rgba(121,192,255,0.08)]",
  flawless:
    REF_SHAPE +
    " text-[#a371f7] border border-[rgba(163,113,247,0.3)] bg-[rgba(163,113,247,0.08)]",
  radiant:
    REF_SHAPE +
    " text-[#d4a847] border border-[rgba(212,168,71,0.4)] bg-[rgba(212,168,71,0.1)]",
};

const RPO_COUNT = "text-[11px] text-[rgba(230,237,243,0.45)]";
const RPO_SCORE = "text-[11px] font-semibold text-[#58a6ff] whitespace-nowrap";

const RPO_ESTIMATED =
  "flex items-center gap-[2px] px-[8px] py-[3px] text-[10px] text-[rgba(230,237,243,0.55)]";
const RPO_EST_SEP = "text-[rgba(230,237,243,0.25)]";

const RPO_REWARDS = "flex flex-col";
const RPO_REWARD =
  "grid grid-cols-[14px_14px_1fr_auto_auto_auto] items-center gap-[3px] px-[8px] py-[2px] border-b border-b-[rgba(48,54,61,0.25)] text-[10px] last:border-b-0";
const RPO_VAULT = "text-[9px] text-center leading-none";
const RPO_OWNED = "text-[10px] font-bold text-center";
const RPO_OWNED_YES = RPO_OWNED + " text-[rgba(63,185,80,0.85)]";
const RPO_OWNED_NO = RPO_OWNED + " text-[rgba(230,237,243,0.25)]";
const RPO_REWARD_NAME = "whitespace-nowrap overflow-hidden text-ellipsis opacity-[0.85]";
const RARITY_NAME: Record<string, string> = {
  Bronze: RPO_REWARD_NAME + " text-[#c47d3a]",
  Silver: RPO_REWARD_NAME + " text-[#9ba8b5]",
  Gold: RPO_REWARD_NAME + " text-[#d4a847]",
};
const RPO_VAL_SHARED = "flex items-center gap-[1px] whitespace-nowrap tabular-nums";
const RPO_PLAT_VAL = RPO_VAL_SHARED + " text-[rgba(121,192,255,0.8)]";
const RPO_DUCAT_VAL = RPO_VAL_SHARED + " text-[rgba(212,168,71,0.75)]";

const RPO_REC_SHAPE =
  "text-[8px] font-bold px-[3px] py-[1px] rounded-[2px] whitespace-nowrap border";
const RPO_REC: Record<string, string> = {
  intact: RPO_REC_SHAPE + " text-[rgba(230,237,243,0.4)] border-[rgba(230,237,243,0.15)]",
  exceptional:
    RPO_REC_SHAPE +
    " text-[#79c0ff] border-[rgba(121,192,255,0.3)] bg-[rgba(121,192,255,0.07)]",
  radiant:
    RPO_REC_SHAPE +
    " text-[#d4a847] border-[rgba(212,168,71,0.35)] bg-[rgba(212,168,71,0.08)]",
};

const ERA_LABEL: Record<string, string> = {
  LITH: "Lith", MESO: "Meso", NEO: "Neo", AXI: "Axi", ALL: "All Eras",
};

const REWARD_ORDER: Record<string, number> = { Gold: 0, Silver: 1, Bronze: 2 };

// Bronze → run Intact (refining reduces common drop rate)
// Silver → Exceptional (solid improvement, low trace cost)
// Gold   → Radiant (rare items benefit most from full refinement)
function recRefinement(rarity: string): string {
  if (rarity === "Gold")   return "Radiant";
  if (rarity === "Silver") return "Exceptional";
  return "Intact";
}

function scoreOf(relic: RelicPickRelic, priority: RelicPickPriority): number {
  if (priority === "platinum") return relic.plat_score;
  if (priority === "ducat")    return relic.ducat_score;
  return relic.unowned_score;
}

function getDisplayRewards(relic: RelicPickRelic, lines: RelicPickLines, priority: RelicPickPriority): RelicPickReward[] {
  const byRarity = [...relic.rewards].sort(
    (a, b) => (REWARD_ORDER[a.rarity] ?? 3) - (REWARD_ORDER[b.rarity] ?? 3)
  );
  if (lines === "all" || lines === "estimated") return byRarity;

  // "best" mode
  if (priority === "platinum") {
    return [...relic.rewards].sort((a, b) => b.plat - a.plat).slice(0, 1);
  }
  if (priority === "ducat") {
    return [...relic.rewards].sort((a, b) => b.ducats - a.ducats).slice(0, 1);
  }
  // "unowned" best: all unowned items ranked by drop probability
  return relic.rewards
    .filter(r => !r.owned)
    .sort((a, b) => b.drop_rate - a.drop_rate);
}

function PlatIcon() {
  return <img src="/platinum.webp" alt="p" width={11} height={11}
    className="mb-px shrink-0 object-contain align-middle" />;
}
function DucatIcon() {
  return <img src="/ducats.webp" alt="d" width={11} height={11}
    className="mb-px shrink-0 object-contain align-middle" />;
}

export default function RelicPickOverlay() {
  const [payload,  setPayload]  = useState<RelicPickPayload | null>(null);
  // Outline mode (Settings → Overlays → Show Outline): dashed frame instead of relic cards.
  const [outline,  setOutline]  = useState(false);
  const [priority, setPriority] = useState<RelicPickPriority>(DEFAULT_RELIC_PICK_PRIORITY);
  const [lines,    setLines]    = useState<RelicPickLines>(DEFAULT_RELIC_PICK_LINES);
  // Use a callback ref so the ResizeObserver is set up each time the root div
  // mounts (payload goes null→non-null). A plain useRef+useEffect misses this
  // because the root div doesn't exist yet when the effect runs at mount time.
  const roRef   = useRef<ResizeObserver | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  // The scale is a CSS transform, so it does not change the measured layout size.
  // The window must grow by the same factor that the content is drawn at.
  const syncSize = useCallback((layoutHeight: number) => {
    if (layoutHeight <= 0) return;
    const s = overlayScale();
    clampToMonitor(340 * s, layoutHeight * s)
      .then(([w, h]) => getCurrentWindow().setSize(new LogicalSize(Math.round(w), Math.round(h))))
      .catch(() => {});
  }, []);

  const rootCallback = useCallback((el: HTMLDivElement | null) => {
    if (roRef.current) { roRef.current.disconnect(); roRef.current = null; }
    rootRef.current = el;
    if (!el) return;
    // Measure the border box: contentRect excludes the root's padding and
    // border (14 px), which clipped the bottom of the window.
    const ro = new ResizeObserver(() => {
      const node = rootRef.current;
      if (node) syncSize(Math.ceil(node.offsetHeight));
    });
    ro.observe(el);
    roRef.current = ro;
  }, [syncSize]);

  const hide = () => {
    setPayload(null);
    getCurrentWindow().hide().catch(() => {});
  };

  useEffect(() => {
    const unOpen = listen<RelicPickPayload>(TAURI_EVENTS.RELIC_PICK_OPEN, async e => {
      // Real screen open wins over an active outline.
      setOutline(false);
      // Reload settings fresh on every show — the main window may have changed them
      // since this overlay was first mounted at app startup.
      try {
        const json = await invoke<string>(TAURI_COMMANDS.LOAD_SETTINGS);
        if (json) {
          const s = JSON.parse(json) as SettingsFile;
          if (RELIC_PICK_PRIORITY_OPTIONS.includes(s.relicPickPriority)) setPriority(s.relicPickPriority);
          if (RELIC_PICK_LINES_OPTIONS.includes(s.relicPickLines))        setLines(s.relicPickLines);
        }
      } catch {}
      setPayload(e.payload);
    });
    const unClose = listen(TAURI_EVENTS.RELIC_PICK_CLOSE, () => hide());
    // A scale change does not alter the layout size, so the ResizeObserver never
    // fires. Measure again to resize a window that is already open.
    const unScale = listen(TAURI_EVENTS.SETTINGS_UPDATED, () => {
      const el = rootRef.current;
      if (el) syncSize(Math.ceil(el.offsetHeight));
    });
    const unOutline = listen<string>(TAURI_EVENTS.OVERLAY_OUTLINE, e => {
      if (e.payload === "relicPick") setOutline(true);
      else if (e.payload === "off-relicPick") { setOutline(false); setPayload(null); }
    });
    return () => { unOpen.then(f => f()); unClose.then(f => f()); unScale.then(f => f()); unOutline.then(f => f()); };
  }, [syncSize]);

  if (outline) {
    return (
      <div ref={rootCallback} className="box-border flex h-[300px] w-full items-center justify-center rounded-[10px] border-2 border-dashed border-[rgba(56,139,253,.85)] bg-[rgba(22,27,34,.55)] text-base font-semibold text-[#79b8ff]">
        Relic Pick Overlay — outline
      </div>
    );
  }

  if (!payload) return null;

  const sorted = [...payload.relics]
    .sort((a, b) => scoreOf(b, priority) - scoreOf(a, priority))
    .slice(0, 3);
  const eraLabel = ERA_LABEL[payload.era] ?? payload.era;

  return (
    <div className={RPO_ROOT} ref={rootCallback}>
      <div className={RPO_HEADER}>
        <span className={RPO_TITLE}>{eraLabel} Fissure</span>
        <button className={RPO_CLOSE} onClick={hide} title="Close">✕</button>
      </div>

      {sorted.length === 0 ? (
        <div className={RPO_EMPTY}>No {eraLabel} relics in inventory</div>
      ) : (
        <div className={RPO_LIST}>
          {sorted.map((relic, i) => {
            const displayRewards = getDisplayRewards(relic, lines, priority);
            const score = scoreOf(relic, priority);
            const scoreLabel = priority === "platinum"
              ? `${score.toFixed(0)}p EV`
              : priority === "ducat"
              ? `${score.toFixed(0)}⬡ EV`
              : `${(score * 100).toFixed(0)}% new`;

            return (
              <div key={relic.name} className={i === 0 ? RPO_CARD_BEST : RPO_CARD}>
                <div className={RPO_CARD_HEADER}>
                  <span className={i === 0 ? RPO_RANK_BEST : RPO_RANK}>#{i + 1}</span>
                  <span className={RPO_RELIC_NAME}>{relic.base_name}</span>
                  <span className={REF_BADGE[relic.refinement] ?? REF_BADGE.intact}>
                    {relic.refinement.charAt(0).toUpperCase() + relic.refinement.slice(1, relic.refinement === "exceptional" ? 5 : 4)}.
                  </span>
                  <span className={RPO_COUNT}>×{relic.count}</span>
                  <span className={RPO_SCORE}>{scoreLabel}</span>
                </div>

                {lines === "estimated" ? (
                  <div className={RPO_ESTIMATED}>
                    <span>{relic.plat_score.toFixed(0)}</span><PlatIcon />
                    <span className={RPO_EST_SEP}> · </span>
                    <span>{relic.ducat_score.toFixed(0)}</span><DucatIcon />
                    <span className={RPO_EST_SEP}> · </span>
                    <span>{relic.rewards.filter(r => !r.owned).length}/{relic.rewards.length} new</span>
                  </div>
                ) : (
                  <div className={RPO_REWARDS}>
                    {displayRewards.map(reward => (
                      <div key={reward.name} className={RPO_REWARD}>
                        <span className={RPO_VAULT}>{reward.vaulted ? "🔒" : " "}</span>
                        <span className={reward.owned ? RPO_OWNED_YES : RPO_OWNED_NO}>
                          {reward.owned ? "✓" : "✗"}
                        </span>
                        <span className={RARITY_NAME[reward.rarity] ?? RPO_REWARD_NAME}>{reward.name}</span>
                        {reward.plat > 0 && (
                          <span className={RPO_PLAT_VAL}>
                            {reward.plat}<PlatIcon />
                          </span>
                        )}
                        {reward.ducats > 0 && (
                          <span className={RPO_DUCAT_VAL}>
                            {reward.ducats}<DucatIcon />
                          </span>
                        )}
                        <span className={RPO_REC[recRefinement(reward.rarity).toLowerCase()] ?? RPO_REC.intact}>
                          {recRefinement(reward.rarity)}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
