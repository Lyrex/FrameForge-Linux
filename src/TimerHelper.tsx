import React, { useState, useEffect, useCallback } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { InventoryItem } from "./types/items";
import type { WorldState, WsFissure, WsStorm } from "./types/worldstate";
import { useWorldState } from "./worldstate";
import { ensurePermission } from "./lib/notify";
import { matchesWatch } from "./fissureAlerts";
import type { FissureWatch } from "./types/settings";

export { matchesWatch };

// ── Tailwind class constants (formerly TimerHelper.css) ───────────────────────

const TH_ROOT = "flex flex-col overflow-y-auto flex-1 min-h-0 pb-[8px]";
const TH_MSG = "px-[16px] py-[12px] text-[12px] text-muted text-center";
const TH_ERROR = "px-[16px] py-[8px] text-[12px] text-danger flex items-center gap-[8px]";
const TH_ERROR_BTN =
  "bg-transparent border border-[var(--red)] text-[var(--red)] text-[11px] px-[8px] py-[1px] rounded-[3px] cursor-pointer";
const TH_GROUP =
  "flex items-center gap-[8px] px-[16px] pt-[6px] pb-[3px] text-[10px] font-bold text-muted uppercase tracking-[0.04em] bg-surface sticky top-0 z-[1] border-b border-b-[rgba(48,54,61,.4)]";
const TH_GROUP_FISSURES = TH_GROUP + " justify-between";

const TH_STAR =
  "bg-transparent border-0 cursor-pointer text-[14px] text-muted px-[2px] py-0 shrink-0 leading-none transition-colors duration-100 hover:text-[#f0c040]";
const TH_STAR_FAV =
  "bg-transparent border-0 cursor-pointer text-[14px] px-[2px] py-0 shrink-0 leading-none transition-colors duration-100 text-[#f0c040]";

// State badges: base bg wins over the legacy `.st-*` backgrounds (later rule in
// the old stylesheet), so only the state colour differs per variant.
const TH_STATE =
  "text-[10px] font-semibold px-[5px] py-[1px] rounded-[3px] bg-[rgba(255,255,255,.07)] shrink-0";
const STATE_COLORS: Record<string, string> = {
  "st-day": "text-[#f0c040]",
  "st-night": "text-[#6eb6ff]",
  "st-warm": "text-[#ff8c5a]",
  "st-cold": "text-[#6eb6ff]",
  "st-fass": "text-[#ff6b4a]",
  "st-duviri": "text-[#c084fc]",
  "st-joy": "text-[#f0c040]",
  "st-anger": "text-[#f87171]",
  "st-envy": "text-[#4ade80]",
  "st-sorrow": "text-[#60a5fa]",
  "st-fear": "text-[#c084fc]",
  "st-active": "text-[var(--green)]",
  "st-away": "text-[var(--muted)]",
  "st-neutral": "text-[var(--muted)]",
};

const TH_SECTION =
  "grid grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-[4px] px-[8px] pt-[4px] pb-[6px]";

const TH_TILE =
  "flex items-start gap-[4px] bg-[rgba(255,255,255,.04)] border border-[rgba(48,54,61,.5)] rounded-[5px] pt-[5px] pr-[7px] pb-[5px] pl-[5px] min-w-0 transition-colors duration-100 hover:bg-[rgba(255,255,255,.07)]";
const TH_TILE_INNER = "flex-1 min-w-0 flex flex-col gap-[3px]";
const TH_TILE_TOP = "flex justify-between items-center gap-[4px]";
const TH_TILE_NAME =
  "text-[11px] font-semibold text-foreground whitespace-nowrap overflow-hidden text-ellipsis min-w-0";
const TH_TILE_CD =
  "text-[11px] font-bold tabular-nums text-foreground whitespace-nowrap";
const TH_TILE_BOTTOM = "flex items-center gap-[5px]";
const TH_TILE_UNTIL =
  "text-[10px] text-muted whitespace-nowrap overflow-hidden text-ellipsis min-w-0";

// ── News & promotions ──
const TH_NEWS_TILE =
  "bg-[rgba(255,255,255,.04)] border border-[rgba(48,54,61,.5)] rounded-[5px] py-[7px] px-[10px] flex flex-col gap-[5px] min-w-0";
const TH_NEWS_MSG = "text-[12px] font-semibold text-foreground leading-[1.35] line-clamp-2";
const TH_NEWS_META = "flex items-center gap-[6px]";
const TH_NEWS_DATE = "text-[10px] text-muted shrink-0";
const TH_NEWS_TAGS = "flex gap-[4px] flex-1 min-w-0";
const TH_NEWS_TAG = "text-[10px] font-semibold px-[5px] py-[1px] rounded-[3px] shrink-0";
const NEWS_TAGS: Record<string, string> = {
  "news-tag-prime":
    TH_NEWS_TAG + " bg-[rgba(230,175,50,.18)] text-[#e6af32] border border-[rgba(230,175,50,.35)]",
  "news-tag-stream":
    TH_NEWS_TAG + " bg-[rgba(145,71,255,.18)] text-[#a47bff] border border-[rgba(145,71,255,.35)]",
  "news-tag-update":
    TH_NEWS_TAG + " bg-[rgba(48,160,255,.18)] text-[#4ea8f0] border border-[rgba(48,160,255,.35)]",
};
const TH_NEWS_BTN =
  "bg-transparent border border-[rgba(48,54,61,.6)] rounded-[3px] text-muted text-[10px] px-[6px] py-[2px] cursor-pointer shrink-0 transition-colors duration-100 whitespace-nowrap hover:bg-[rgba(255,255,255,.08)] hover:text-foreground hover:border-[rgba(139,148,158,.5)]";

// ── Alerts ──
const TH_ALERT_TILE =
  "bg-[rgba(255,255,255,.04)] border border-[rgba(48,54,61,.5)] rounded-[5px] px-[8px] py-[6px] flex flex-col gap-[3px] min-w-0";
const TH_ALERT_TOP = "flex justify-between items-center gap-[4px]";
const TH_ALERT_TYPE =
  "text-[12px] font-semibold text-foreground whitespace-nowrap overflow-hidden text-ellipsis min-w-0";
const TH_ALERT_CD =
  "text-[11px] font-bold tabular-nums text-foreground whitespace-nowrap shrink-0";
const TH_ALERT_BOTTOM = "flex gap-[5px] items-center";
const TH_ALERT_FACTION =
  "text-[10px] text-muted bg-[rgba(255,255,255,.06)] px-[4px] py-0 rounded-[2px] shrink-0";
const TH_ALERT_REWARD =
  "text-[10px] text-[#f0c040] whitespace-nowrap overflow-hidden text-ellipsis min-w-0";

// ── Invasions ──
const TH_INV_TILE =
  "bg-[rgba(255,255,255,.04)] border border-[rgba(48,54,61,.5)] rounded-[5px] px-[8px] py-[6px] flex flex-col gap-[2px] min-w-0";
const TH_INV_NODE =
  "text-[11px] font-semibold text-foreground whitespace-nowrap overflow-hidden text-ellipsis";
const TH_INV_FACTIONS = "flex items-center justify-between gap-[4px] text-[10px]";
const TH_INV_REWARD =
  "text-muted whitespace-nowrap overflow-hidden text-ellipsis text-right min-w-0 max-w-[50%]";
const TH_INV_BAR_WRAP =
  "h-[3px] bg-[rgba(255,255,255,.08)] mx-[28px] my-0 rounded-[2px] overflow-hidden";
const TH_INV_BAR_INNER = "h-full bg-accent rounded-[2px] transition-[width] duration-300";
const TH_INV_ATT = "text-[#f85149]";
const TH_INV_DEF = "text-[#6eb6ff]";

// ── Fissures ──
const TH_FISSURE_GRID =
  "grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-[5px] pt-[6px] px-[8px] pb-[8px]";
const TH_FISSURE_TILE =
  "bg-[rgba(255,255,255,.04)] border border-[rgba(48,54,61,.5)] rounded-[5px] px-[8px] py-[6px] flex flex-col gap-[3px] transition-colors duration-100 min-w-0 hover:bg-[rgba(255,255,255,.07)]";
// Watched wins over hover (later rule at equal specificity) → no hover class;
// left edge keeps the standalone `.fissure-watched` 2px accent border.
const TH_FISSURE_WATCHED =
  "bg-[rgba(56,139,253,.08)] border-t border-r border-b border-l-2 border-[var(--accent)] rounded-[5px] px-[8px] py-[6px] flex flex-col gap-[3px] transition-colors duration-100 min-w-0";
const TH_FISSURE_TOP = "flex justify-between items-center gap-[4px]";
const TH_FISSURE_CD =
  "text-[11px] font-bold tabular-nums text-foreground whitespace-nowrap";
const TH_FISSURE_MISSION =
  "text-[12px] text-foreground font-medium whitespace-nowrap overflow-hidden text-ellipsis";
const TH_FISSURE_BOTTOM = "flex gap-[6px] items-center";
const TH_FISSURE_ENEMY =
  "text-[10px] text-muted bg-[rgba(255,255,255,.06)] px-[4px] py-0 rounded-[2px] shrink-0";
const TH_FISSURE_NODE =
  "text-[10px] text-muted whitespace-nowrap overflow-hidden text-ellipsis min-w-0";
const TH_FISSURE_TABS = "flex gap-[2px] ml-auto";
const TH_TAB_BASE =
  "bg-transparent border border-[rgba(48,54,61,.8)] text-muted text-[10px] px-[7px] py-[1px] rounded-[3px] cursor-pointer transition-colors duration-100 hover:bg-[rgba(255,255,255,.1)] hover:text-foreground";
const TH_TAB_ACTIVE =
  "bg-[rgba(255,255,255,.1)] border border-[rgba(48,54,61,.8)] text-foreground text-[10px] px-[7px] py-[1px] rounded-[3px] cursor-pointer transition-colors duration-100";
const TH_WATCH_BTN =
  "bg-transparent border-0 cursor-pointer text-muted px-[4px] py-[2px] rounded-[3px] flex items-center transition-colors duration-100 shrink-0 hover:bg-[rgba(255,255,255,.1)] hover:text-foreground";
const TH_WATCH_BTN_ACTIVE =
  "bg-[rgba(255,255,255,.1)] border-0 cursor-pointer text-foreground px-[4px] py-[2px] rounded-[3px] flex items-center transition-colors duration-100 shrink-0";
const TH_WATCH_SVG = "w-[14px] h-[14px]";

// ── Watch management panel ──
const TH_WATCH_PANEL =
  "pt-[8px] px-[12px] pb-[10px] bg-[rgba(0,0,0,.2)] border-b border-b-[rgba(48,54,61,.5)]";
const TH_WATCH_CHIPS = "flex flex-wrap gap-[5px] mb-[10px]";
const TH_WATCH_CHIP =
  "flex items-center gap-[4px] pt-[2px] pb-[2px] pl-[8px] pr-[6px] bg-[rgba(255,255,255,.07)] border border-[rgba(255,255,255,.1)] rounded-[12px] text-[11px]";
const TH_CHIP_TIER = "font-bold";
const TH_CHIP_MT = "text-muted";
const TH_CHIP_VAR = "text-muted italic";
const TH_CHIP_DEL =
  "bg-transparent border-0 cursor-pointer text-muted text-[14px] pl-[2px] pr-0 py-0 leading-none transition-colors duration-100 hover:text-danger";
const TH_WATCH_FORM = "flex flex-col gap-[6px]";
const TH_FORM_ROW = "flex items-start gap-[8px]";
const TH_FORM_LABEL =
  "text-[10px] font-bold text-muted uppercase tracking-[0.04em] w-[32px] shrink-0 pt-[3px]";
const TH_PILL_GROUP = "flex flex-nowrap gap-[3px] flex-1";
const TH_PILL_GROUP_WRAP = "flex flex-wrap gap-[3px] flex-1";
const TH_PILL =
  "bg-transparent border border-[rgba(48,54,61,.8)] text-muted text-[10px] px-[7px] py-[2px] rounded-[3px] cursor-pointer whitespace-nowrap transition-colors duration-100 hover:bg-[rgba(255,255,255,.07)] hover:text-foreground";
const TH_PILL_ACTIVE =
  "bg-[rgba(56,139,253,.2)] border border-[var(--accent)] text-[var(--accent)] text-[10px] px-[7px] py-[2px] rounded-[3px] cursor-pointer whitespace-nowrap transition-colors duration-100";
const TH_ADD_BTN =
  "self-end bg-[rgba(56,139,253,.15)] border border-[var(--accent)] text-[var(--accent)] text-[11px] px-[12px] py-[3px] rounded-[4px] cursor-pointer mt-[2px] transition-colors duration-100 hover:bg-[rgba(56,139,253,.3)]";
const TH_NOTIFY =
  "flex items-center gap-[5px] mt-[6px] pt-[6px] border-t border-t-[rgba(48,54,61,.5)] text-muted text-[11px] cursor-pointer";
const TH_NOTIFY_INPUT = "m-0 cursor-pointer";
const TH_NOTIFY_DENIED = "mt-[4px] text-danger text-[10px]";

// ── Expandable tiles ──
const TH_EXP_SHELL =
  "bg-[rgba(255,255,255,.04)] border rounded-[5px] overflow-hidden min-w-0 transition-colors duration-100";
const TH_EXP = TH_EXP_SHELL + " border-[rgba(48,54,61,.5)] cursor-pointer hover:bg-[rgba(255,255,255,.07)]";
const TH_EXP_OPEN = TH_EXP_SHELL + " border-[rgba(56,139,253,.4)] col-span-full cursor-pointer hover:bg-[rgba(255,255,255,.07)]";
const TH_EXP_AWAY = TH_EXP_SHELL + " border-[rgba(48,54,61,.5)] cursor-default opacity-[.7] hover:bg-[rgba(255,255,255,.07)]";
const TH_EXP_AWAY_OPEN = TH_EXP_SHELL + " border-[rgba(56,139,253,.4)] col-span-full cursor-default opacity-[.7] hover:bg-[rgba(255,255,255,.07)]";
const TH_EXP_HEADER = "flex flex-col gap-[4px] pt-[6px] px-[8px] pb-[5px]";
const TH_EXP_NAME_ROW = "flex items-center gap-[4px]";
const TH_EXP_META_ROW = "flex items-center gap-[5px]";
const TH_EXP_NAME =
  "text-[11px] font-semibold text-foreground flex-1 min-w-0 whitespace-nowrap overflow-hidden text-ellipsis";
const TH_EXP_CD =
  "text-[11px] font-bold tabular-nums text-foreground whitespace-nowrap shrink-0 ml-auto";
const TH_EXP_CHEVRON = "w-[10px] h-[6px] shrink-0 text-muted transition-transform duration-150";
const TH_EXP_LOCATION = "text-[10px] text-muted pb-[4px] pr-[7px] pl-[28px]";
const TH_EXP_BODY =
  "border-t border-t-[rgba(48,54,61,.4)] pt-[4px] px-[7px] pb-[5px] bg-[rgba(0,0,0,.15)] cursor-default";
// `.exp-tile-inventory` came later in the sheet and won the padding clash.
const TH_EXP_BODY_INV =
  "border-t border-t-[rgba(48,54,61,.4)] pt-[2px] px-[7px] pb-[4px] bg-[rgba(0,0,0,.15)] cursor-default max-h-[200px] overflow-y-auto";
const TH_EXP_ROW =
  "flex items-center gap-[6px] py-[2px] text-[11px] border-b border-b-[rgba(48,54,61,.2)] last:border-b-0";
const TH_EXP_ROW_TYPE = "text-foreground shrink-0";
const TH_EXP_ROW_MOD = "text-[10px] text-[#f0c040] shrink-0";
const TH_EXP_ROW_NODE =
  "text-muted flex-1 min-w-0 whitespace-nowrap overflow-hidden text-ellipsis text-right";

// ── Inventory rows (Baro / Resurgence) ──
const TH_INV_ROW =
  "flex items-center gap-[8px] py-[3px] border-b border-b-[rgba(48,54,61,.2)] text-[11px] last:border-b-0";
const TH_INV_NAME =
  "flex-1 text-foreground whitespace-nowrap overflow-hidden text-ellipsis min-w-0";
const TH_INV_NAME_OWNED =
  "flex-1 text-[var(--green)] whitespace-nowrap overflow-hidden text-ellipsis min-w-0";
const TH_OWNED_TAG =
  "text-[9px] font-bold text-[var(--green)] bg-[rgba(63,185,80,.15)] border border-[rgba(63,185,80,.3)] px-[4px] py-0 rounded-[3px] shrink-0 whitespace-nowrap";
const TH_PRICE = "shrink-0 font-bold tabular-nums whitespace-nowrap";
const TH_PRICE_AYA = TH_PRICE + " text-[#f0c040]";
const TH_PRICE_CR = TH_PRICE + " text-muted";
const TH_CURRENCY = "text-[9px] font-normal opacity-80";

// ── Helpers ───────────────────────────────────────────────────────────────────

export function fmtMs(ms: number): string {
  if (ms <= 0) return "—";
  const s = Math.floor(ms / 1000) % 60;
  const m = Math.floor(ms / 60000) % 60;
  const h = Math.floor(ms / 3600000) % 24;
  const d = Math.floor(ms / 86400000);
  if (d > 0) return `${d}d ${h}h ${m}m`;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  return `${m}m ${s}s`;
}

export function fmtExpiry(expiry: string, now: number): string {
  return fmtMs(new Date(expiry).getTime() - now);
}

function nextUtcMidnight(): string {
  const n = new Date();
  return new Date(Date.UTC(n.getUTCFullYear(), n.getUTCMonth(), n.getUTCDate() + 1)).toISOString();
}
function nextWeeklyReset(): string {
  const n = new Date();
  const d = n.getUTCDay();
  return new Date(Date.UTC(n.getUTCFullYear(), n.getUTCMonth(), n.getUTCDate() + (d === 0 ? 1 : 8 - d))).toISOString();
}

const DUVIRI_EPOCH_MS = 1679450400000; // 2023-03-22T02:00:00.000Z
const DUVIRI_MOOD_MS  = 7_200_000;     // 2 hours per mood
const DUVIRI_MOODS    = ["Joy", "Anger", "Envy", "Sorrow", "Fear", "Neutral"] as const;
type DuviriMood = typeof DUVIRI_MOODS[number];
const DUVIRI_MOOD_CLASS: Record<DuviriMood, string> = {
  Joy: "st-joy", Anger: "st-anger", Envy: "st-envy",
  Sorrow: "st-sorrow", Fear: "st-fear", Neutral: "st-neutral",
};
function duviriNow(now: number): { mood: DuviriMood; expiry: string; index: number } {
  const idx   = Math.floor((now - DUVIRI_EPOCH_MS) / DUVIRI_MOOD_MS) % DUVIRI_MOODS.length;
  const next  = DUVIRI_EPOCH_MS + (Math.floor((now - DUVIRI_EPOCH_MS) / DUVIRI_MOOD_MS) + 1) * DUVIRI_MOOD_MS;
  return { mood: DUVIRI_MOODS[idx], expiry: new Date(next).toISOString(), index: idx };
}

const TIER_COLOR: Record<string, string> = {
  Lith: "#c8853a", Meso: "#a8a9ad", Neo: "#f0c040",
  Axi: "#e5c04a", Requiem: "#9b6dff", Omnia: "#e0e0e0",
};

export function getTimerInfo(id: string, ws: WorldState): { state: string; expiry: string } | null {
  switch (id) {
    case "cetus-cycle":    return ws.cetus    ? { state: ws.cetus.isDay ? "Day" : "Night",        expiry: ws.cetus.expiry }    : null;
    case "vallis-cycle":   return ws.vallis   ? { state: ws.vallis.isWarm ? "Warm" : "Cold",      expiry: ws.vallis.expiry }   : null;
    case "cambion-cycle":  return ws.cambion  ? { state: "Cycle",                                 expiry: ws.cambion.expiry }  : null;
    case "zariman-cycle":  return ws.zariman  ? { state: "Active",                                expiry: ws.zariman.expiry }  : null;
    case "bounty-cetus":   return ws.bounties?.cetus    ? { state: `${ws.bounties.cetus.jobCount} jobs`,    expiry: ws.bounties.cetus.expiry }    : null;
    case "bounty-vallis":  return ws.bounties?.vallis   ? { state: `${ws.bounties.vallis.jobCount} jobs`,   expiry: ws.bounties.vallis.expiry }   : null;
    case "bounty-cambion": return ws.bounties?.cambion  ? { state: `${ws.bounties.cambion.jobCount} jobs`,  expiry: ws.bounties.cambion.expiry }  : null;
    case "bounty-zariman": return ws.bounties?.zariman  ? { state: `${ws.bounties.zariman.jobCount} jobs`,  expiry: ws.bounties.zariman.expiry }  : null;
    case "bounty-hex":     return ws.bounties?.hex      ? { state: `${ws.bounties.hex.jobCount} jobs`,      expiry: ws.bounties.hex.expiry }      : null;
    case "sortie":         return ws.sortie      ? { state: ws.sortie.faction,     expiry: ws.sortie.expiry }     : null;
    case "archon-hunt":    return ws.archonHunt  ? { state: ws.archonHunt.boss,    expiry: ws.archonHunt.expiry } : null;
    case "daily-reset":    return { state: "UTC 00:00", expiry: nextUtcMidnight() };
    case "weekly-reset":   return { state: "Monday",    expiry: nextWeeklyReset() };
    case "void-trader":    return ws.voidTrader ? { state: ws.voidTrader.active ? "Here" : "Away", expiry: ws.voidTrader.active ? ws.voidTrader.expiry : ws.voidTrader.activation } : null;
    case "nightwave":         return ws.nightwave?.active ? { state: `S${ws.nightwave.season}`, expiry: ws.nightwave.expiry } : null;
    case "prime-resurgence":  return ws.primeResurgence?.active ? { state: "Active", expiry: ws.primeResurgence.expiry } : null;
    case "duviri-spiral": { const d = duviriNow(Date.now()); return { state: d.mood, expiry: d.expiry }; }
    case "circuit":        return ws.circuit    ? { state: "Weekly",  expiry: ws.circuit.expiry }        : null;
    case "kahl":           return ws.kahl       ? { state: "Weekly",  expiry: ws.kahl.expiry }           : null;
    case "deep-archimedea":return ws.deepArchimedea ? { state: "Weekly", expiry: ws.deepArchimedea.expiry } : null;
    default: return null;
  }
}

// ── Component ─────────────────────────────────────────────────────────────────

const TIERS = ["Any","Omnia","Lith","Meso","Neo","Axi","Requiem"];
const VARIANTS: { key: FissureWatch["variant"]; label: string }[] = [
  { key: "any",    label: "Any" },
  { key: "normal", label: "Normal" },
  { key: "hard",   label: "Steel Path" },
  { key: "storm",  label: "Storm" },
];
// Mission types available per variant (storms are Railjack — different pool)
const MISSION_TYPES_BY_VARIANT: Record<FissureWatch["variant"], string[]> = {
  any:    ["Any","Rescue","Capture","Defense","Survival","Excavation","Interception","Disruption","Sabotage","Spy","Mobile Defense","Extermination","Assassination","Assault","Void Cascade","Void Flood","Alchemy","Skirmish","Volatile"],
  normal: ["Any","Rescue","Capture","Defense","Survival","Excavation","Interception","Disruption","Sabotage","Spy","Mobile Defense","Extermination","Assassination","Assault","Void Cascade","Void Flood","Alchemy"],
  hard:   ["Any","Rescue","Capture","Defense","Survival","Excavation","Interception","Disruption","Sabotage","Spy","Mobile Defense","Extermination","Assassination","Assault","Void Cascade","Void Flood","Alchemy"],
  storm:  ["Any","Skirmish","Volatile","Defense","Extermination","Sabotage","Assassination"],
};

interface Props {
  active: boolean;
  favorites: string[];
  onFavoriteToggle: (id: string) => void;
  fissureWatches: FissureWatch[];
  onAddWatch: (w: FissureWatch) => void;
  onRemoveWatch: (id: string) => void;
  fissureNotifications: boolean;
  onFissureNotificationsChange: (enabled: boolean) => void;
  inventory: Record<string, InventoryItem>;
}

type FissureTab = "normal" | "hard" | "storm";

export default function TimerHelper({ active, favorites, onFavoriteToggle, fissureWatches, onAddWatch, onRemoveWatch, fissureNotifications, onFissureNotificationsChange, inventory }: Props) {
  const { worldState: ws, error, refresh: fetchWS } = useWorldState();
  const [now, setNow] = useState(Date.now());
  const loading = !ws && !error;
  const [fissureTab, setFissureTab] = useState<FissureTab>("normal");
  const [showWatchForm, setShowWatchForm] = useState(false);
  const [permissionDenied, setPermissionDenied] = useState(false);
  const [openInventory, setOpenInventory] = useState<Set<string>>(new Set());
  const toggleInventory = (id: string) => setOpenInventory(prev => {
    const next = new Set(prev);
    next.has(id) ? next.delete(id) : next.add(id);
    return next;
  });
  const [wTier, setWTier] = useState("Any");
  const [wMission, setWMission] = useState("Any");
  const [wVariant, setWVariant] = useState<FissureWatch["variant"]>("any");

  const setVariant = useCallback((v: FissureWatch["variant"]) => {
    setWVariant(v);
    // Reset mission if it's not available in the new variant's list
    setWMission(m => MISSION_TYPES_BY_VARIANT[v].includes(m) ? m : "Any");
  }, []);

  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [active]);

  // Watches loaded from settings never went through the add-watch button, so
  // without this nothing would ever ask the OS and every alert would silently
  // no-op. Opening the tab is the closest thing to a gesture we get for them.
  useEffect(() => {
    if (!fissureNotifications || fissureWatches.length === 0) return;
    ensurePermission().then(granted => setPermissionDenied(!granted));
  }, []); // eslint-disable-line

  const isFav = (id: string) => favorites.includes(id);
  const cd = (expiry: string) => fmtMs(new Date(expiry).getTime() - now);

  function StarBtn({ id }: { id: string }) {
    return (
      <button
        className={isFav(id) ? TH_STAR_FAV : TH_STAR}
        onClick={e => { e.stopPropagation(); onFavoriteToggle(id); }}
        title={isFav(id) ? "Unpin" : "Pin to Modular Window"}
      >
        {isFav(id) ? "★" : "☆"}
      </button>
    );
  }

  const Chevron = ({ open }: { open: boolean }) => (
    <svg className={`${TH_EXP_CHEVRON} ${open ? "" : "-rotate-90"}`} viewBox="0 0 10 6" fill="none">
      <path d="M1 1L5 5L9 1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
    </svg>
  );

  function ExpHeader({ id, name, state, sc, countdown, open, showChevron = true }: {
    id?: string; name: string; state: string; sc?: string; countdown: string; open: boolean; showChevron?: boolean;
  }) {
    return (
      <div className={TH_EXP_HEADER}>
        <div className={TH_EXP_NAME_ROW}>
          <span className={TH_EXP_NAME}>{name}</span>
          {showChevron && <Chevron open={open} />}
        </div>
        <div className={TH_EXP_META_ROW}>
          {id && <StarBtn id={id} />}
          <span className={TH_STATE + " " + (STATE_COLORS[sc ?? "st-neutral"] ?? "")}>{state}</span>
          <span className={TH_EXP_CD}>{countdown}</span>
        </div>
      </div>
    );
  }

  function SectionHeader({ label, children }: { label: string; children?: React.ReactNode }) {
    return <div className={TH_GROUP}>{label}{children}</div>;
  }

  // Compact tile for use inside a 2-column grid
  function TimerTile({ id, label, state, stateClass, expiry, until }: {
    id?: string; label: string; state: string; stateClass?: string; expiry: string; until?: string;
  }) {
    return (
      <div className={TH_TILE}>
        {id && <StarBtn id={id} />}
        <div className={TH_TILE_INNER}>
          <div className={TH_TILE_TOP}>
            <span className={TH_TILE_NAME}>{label}</span>
            <span className={TH_TILE_CD}>{fmtMs(new Date(expiry).getTime() - now)}</span>
          </div>
          <div className={TH_TILE_BOTTOM}>
            <span className={TH_STATE + " " + (STATE_COLORS[stateClass ?? "st-neutral"] ?? "")}>{state}</span>
            {until && <span className={TH_TILE_UNTIL}>{until}</span>}
          </div>
        </div>
      </div>
    );
  }

  if (loading) return <div className={TH_MSG}>Loading worldstate…</div>;

  return (
    <div className={TH_ROOT}>
      {error && <div className={TH_ERROR}>{error} <button className={TH_ERROR_BTN} onClick={fetchWS}>Retry</button></div>}

      {/* ── World Cycles ──────────────────────────────────────────────────── */}
      <SectionHeader label="World Cycles" />
      <div className={TH_SECTION}>
        {ws?.cetus    && <TimerTile id="cetus-cycle"   label="Cetus"         state={ws.cetus.isDay ? "Day" : "Night"}     stateClass={ws.cetus.isDay ? "st-day" : "st-night"}  expiry={ws.cetus.expiry}   until={`until ${ws.cetus.isDay ? "Night" : "Day"}`} />}
        {ws?.vallis   && <TimerTile id="vallis-cycle"  label="Orb Vallis"    state={ws.vallis.isWarm ? "Warm" : "Cold"}   stateClass={ws.vallis.isWarm ? "st-warm" : "st-cold"} expiry={ws.vallis.expiry}  until={`until ${ws.vallis.isWarm ? "Cold" : "Warm"}`} />}
        {ws?.cambion  && <TimerTile id="cambion-cycle" label="Cambion Drift" state="Active"                               stateClass="st-fass"                                 expiry={ws.cambion.expiry} until="next cycle" />}
        {ws?.zariman  && <TimerTile id="zariman-cycle" label="Zariman"       state="Active"                               stateClass="st-neutral"                              expiry={ws.zariman.expiry} until="reset" />}
        {(() => { const dv = duviriNow(now); const next = DUVIRI_MOODS[(dv.index + 1) % DUVIRI_MOODS.length]; return <TimerTile id="duviri-cycle" label="Duviri Spiral" state={dv.mood} stateClass={DUVIRI_MOOD_CLASS[dv.mood]} expiry={dv.expiry} until={`until ${next}`} />; })()}
      </div>

      {/* ── Bounties ──────────────────────────────────────────────────────── */}
      {ws?.bounties && Object.keys(ws.bounties).length > 0 && (
        <>
          <SectionHeader label="Bounties" />
          <div className={TH_SECTION}>
            {([
              ["cetus",   "Cetus",          "bounty-cetus"],
              ["vallis",  "Orb Vallis",     "bounty-vallis"],
              ["cambion", "Cambion Drift",  "bounty-cambion"],
              ["zariman", "Zariman",        "bounty-zariman"],
              ["hex",     "Hex / Albrecht", "bounty-hex"],
            ] as [string, string, string][]).map(([key, label, favId]) => {
              const b = ws.bounties![key];
              if (!b) return null;
              return <TimerTile key={key} id={favId} label={label} state={`${b.jobCount} jobs`} stateClass="st-neutral" expiry={b.expiry} until="reset" />;
            })}
          </div>
        </>
      )}

      {/* ── Daily & Weekly ────────────────────────────────────────────────── */}
      <SectionHeader label="Daily &amp; Weekly" />
      <div className={TH_SECTION}>
        <TimerTile id="daily-reset"  label="Daily Reset"  state="UTC 00:00" stateClass="st-neutral" expiry={nextUtcMidnight()} />
        <TimerTile id="weekly-reset" label="Weekly Reset" state="Monday"    stateClass="st-neutral" expiry={nextWeeklyReset()} />
        {ws?.kahl           && <TimerTile id="kahl"            label="Kahl / Break Narmer" state="Weekly" stateClass="st-neutral" expiry={ws.kahl.expiry} />}
        {ws?.deepArchimedea && <TimerTile id="deep-archimedea" label="Deep Archimedea"      state="Weekly" stateClass="st-neutral" expiry={ws.deepArchimedea.expiry} />}
      </div>

      {/* ── Daily & Weekly — expandable tiles ─────────────────────────────── */}
      <div className={TH_SECTION}>
        {ws?.sortie && (() => {
          const open = openInventory.has("sortie");
          return (
            <div className={open ? TH_EXP_OPEN : TH_EXP} onClick={() => toggleInventory("sortie")}>
              <ExpHeader id="sortie" name="Sortie" state={ws.sortie.faction} countdown={cd(ws.sortie.expiry)} open={open} />
              {open && <div className={TH_EXP_BODY}>
                {ws.sortie.variants?.map((v, i) => (
                  <div key={i} className={TH_EXP_ROW}>
                    <span className={TH_EXP_ROW_TYPE}>{v.missionType}</span>
                    {v.modifier && <span className={TH_EXP_ROW_MOD}>{v.modifier}</span>}
                    <span className={TH_EXP_ROW_NODE}>{v.node}</span>
                  </div>
                ))}
              </div>}
            </div>
          );
        })()}

        {ws?.archonHunt && (() => {
          const open = openInventory.has("archon");
          return (
            <div className={open ? TH_EXP_OPEN : TH_EXP} onClick={() => toggleInventory("archon")}>
              <ExpHeader id="archon-hunt" name="Archon Hunt" state={ws.archonHunt.boss} countdown={cd(ws.archonHunt.expiry)} open={open} />
              {open && <div className={TH_EXP_BODY}>
                {ws.archonHunt.missions?.map((m, i) => (
                  <div key={i} className={TH_EXP_ROW}>
                    <span className={TH_EXP_ROW_TYPE}>{m.type}</span>
                    <span className={TH_EXP_ROW_NODE}>{m.node}</span>
                  </div>
                ))}
              </div>}
            </div>
          );
        })()}

        {ws?.circuit && (() => {
          const open = openInventory.has("circuit");
          return (
            <div className={open ? TH_EXP_OPEN : TH_EXP} onClick={() => toggleInventory("circuit")}>
              <ExpHeader id="circuit" name="The Circuit" state="Duviri" sc="st-duviri" countdown={cd(ws.circuit.expiry)} open={open} />
              {open && <div className={TH_EXP_BODY}>
                {ws.circuit.normalFrames?.length > 0 && <div className={TH_EXP_ROW}><span className={TH_EXP_ROW_MOD}>Normal</span><span className={TH_EXP_ROW_NODE}>{ws.circuit.normalFrames.join(" · ")}</span></div>}
                {ws.circuit.hardWeapons?.length > 0  && <div className={TH_EXP_ROW}><span className={TH_EXP_ROW_MOD}>Hard</span><span className={TH_EXP_ROW_NODE}>{ws.circuit.hardWeapons.join(" · ")}</span></div>}
              </div>}
            </div>
          );
        })()}
      </div>

      {/* ── Events ────────────────────────────────────────────────────────── */}
      <SectionHeader label="Events" />
      <div className={TH_SECTION}>
        {ws?.voidTrader && (() => {
          const open = openInventory.has("baro");
          const active = ws.voidTrader.active;
          return (
            <div className={open ? (active ? TH_EXP_OPEN : TH_EXP_AWAY_OPEN) : (active ? TH_EXP : TH_EXP_AWAY)} onClick={() => active && toggleInventory("baro")}>
              <ExpHeader id="void-trader" name={ws.voidTrader.character} state={active ? "Here" : "Away"} sc={active ? "st-active" : "st-away"} countdown={active ? cd(ws.voidTrader.expiry) : fmtMs(new Date(ws.voidTrader.activation).getTime() - now)} open={open} showChevron={active} />
              {active && !open && <div className={TH_EXP_LOCATION}>{ws.voidTrader.location}</div>}
              {active && open && <div className={TH_EXP_BODY_INV} onClick={e => e.stopPropagation()}>
                {ws.voidTrader.manifest.map((item, i) => {
                  const owned = item.uniqueName ? (inventory[item.uniqueName]?.quantity ?? 0) > 0 : false;
                  return (
                    <div key={i} className={TH_INV_ROW}>
                      <span className={owned ? TH_INV_NAME_OWNED : TH_INV_NAME}>{item.name}</span>
                      {owned && <span className={TH_OWNED_TAG}>Owned</span>}
                      {item.primePrice ? <span className={TH_PRICE_AYA}>{item.primePrice} <span className={TH_CURRENCY}>Ducats</span></span> : null}
                      {item.regularPrice ? <span className={TH_PRICE_CR}>{item.regularPrice?.toLocaleString()} <span className={TH_CURRENCY}>cr</span></span> : null}
                    </div>
                  );
                })}
              </div>}
            </div>
          );
        })()}

        {ws?.primeResurgence?.active && (() => {
          const open = openInventory.has("prime-resurgence");
          return (
            <div className={open ? TH_EXP_OPEN : TH_EXP} onClick={() => toggleInventory("prime-resurgence")}>
              <ExpHeader id="prime-resurgence" name="Prime Resurgence" state="Active" sc="st-active" countdown={cd(ws.primeResurgence!.expiry)} open={open} />
              {open && <div className={TH_EXP_BODY_INV} onClick={e => e.stopPropagation()}>
                {ws.primeResurgence!.manifest.map((item, i) => {
                  const owned = item.uniqueName ? (inventory[item.uniqueName]?.quantity ?? 0) > 0 : false;
                  return (
                    <div key={i} className={TH_INV_ROW}>
                      <span className={owned ? TH_INV_NAME_OWNED : TH_INV_NAME}>{item.name}</span>
                      {owned && <span className={TH_OWNED_TAG}>Owned</span>}
                      {item.regalAyaPrice ? <span className={`${TH_PRICE_AYA} text-[#c084fc]`}>{item.regalAyaPrice} <span className={TH_CURRENCY}>Regal Aya</span></span> : null}
                      {item.ayaPrice ? <span className={TH_PRICE_AYA}>{item.ayaPrice} <span className={TH_CURRENCY}>Aya</span></span> : null}
                    </div>
                  );
                })}
              </div>}
            </div>
          );
        })()}

        {ws?.nightwave?.active && <TimerTile id="nightwave" label="Nightwave" state={`Season ${ws.nightwave.season}`} stateClass="st-neutral" expiry={ws.nightwave.expiry} />}
        {ws?.events?.map(ev => (
          <TimerTile key={ev.label} label={ev.label} state="Event" stateClass="st-active" expiry={ev.expiry} />
        ))}
        {ws?.darvo && <TimerTile label={`Darvo: ${ws.darvo.item}`} state={`-${ws.darvo.discount}%`} stateClass="st-neutral" expiry={ws.darvo.expiry} until={`${ws.darvo.salePrice}p`} />}
      </div>

      {/* ── News & Promotions ─────────────────────────────────────────────── */}
      {ws?.news && ws.news.length > 0 && (
        <>
          <SectionHeader label="News &amp; Promotions" />
          <div className={TH_SECTION}>
            {ws.news.map((item, i) => (
              <div key={i} className={TH_NEWS_TILE}>
                <div className={TH_NEWS_MSG}>{item.message}</div>
                <div className={TH_NEWS_META}>
                  <span className={TH_NEWS_DATE}>
                    {new Date(item.date).toLocaleDateString(undefined, { month: "short", day: "numeric" })}
                  </span>
                  <div className={TH_NEWS_TAGS}>
                    {item.primeAccess && <span className={NEWS_TAGS["news-tag-prime"]}>Prime</span>}
                    {item.stream      && <span className={NEWS_TAGS["news-tag-stream"]}>Stream</span>}
                    {item.update      && <span className={NEWS_TAGS["news-tag-update"]}>Update</span>}
                  </div>
                  <button className={TH_NEWS_BTN} onClick={() => openUrl(item.link)}>Open →</button>
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {/* ── Alerts ────────────────────────────────────────────────────────── */}
      {ws?.alerts && ws.alerts.length > 0 && (
        <>
          <SectionHeader label={`Alerts (${ws.alerts.length})`} />
          <div className={TH_SECTION}>
            {ws.alerts.map((a, i) => (
              <div key={i} className={TH_ALERT_TILE}>
                <div className={TH_ALERT_TOP}>
                  <span className={TH_ALERT_TYPE}>{a.missionType}</span>
                  <span className={TH_ALERT_CD}>{cd(a.expiry)}</span>
                </div>
                <div className={TH_ALERT_BOTTOM}>
                  <span className={TH_ALERT_FACTION}>{a.faction}</span>
                  {a.rewardItem && <span className={TH_ALERT_REWARD}>{a.rewardItem}</span>}
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {/* ── Invasions ─────────────────────────────────────────────────────── */}
      {ws?.invasions && ws.invasions.length > 0 && (
        <>
          <SectionHeader label={`Invasions (${ws.invasions.length})`} />
          <div className={TH_SECTION}>
            {ws.invasions.map((inv, i) => (
              <div key={i} className={TH_INV_TILE}>
                <div className={TH_INV_NODE}>{inv.node}</div>
                <div className={`${TH_INV_BAR_WRAP} my-[3px]`}>
                  <div className={TH_INV_BAR_INNER} style={{ width: `${Math.min(100, inv.pct)}%` }} />
                </div>
                <div className={TH_INV_FACTIONS}>
                  <span className={TH_INV_ATT}>{inv.attacker}</span>
                  <span className={TH_INV_REWARD}>{inv.attReward || "—"}</span>
                </div>
                <div className={TH_INV_FACTIONS}>
                  <span className={TH_INV_DEF}>{inv.defender}</span>
                  <span className={TH_INV_REWARD}>{inv.defReward || "—"}</span>
                </div>
              </div>
            ))}
          </div>
        </>
      )}

      {/* ── Void Fissures ─────────────────────────────────────────────────── */}
      {(() => {
        // Rust already filters: started (activation ≤ now) and not expired (expiry > now).
        // Just sort — no extra client-side filtering so count always equals what renders.
        const normal = [...(ws?.fissures   ?? [])].sort((a, b) => a.tierNum - b.tierNum);
        const hard   = [...(ws?.spFissures ?? [])].sort((a, b) => a.tierNum - b.tierNum);
        const storms = [...(ws?.voidStorms ?? [])].sort((a, b) => a.tierNum - b.tierNum);
        const list   = fissureTab === "normal" ? normal : fissureTab === "hard" ? hard : storms;

        const FissureTile = ({ f, v }: { f: WsFissure | WsStorm; v: "normal" | "hard" | "storm" }) => (
          <div className={fissureWatches.some(w => matchesWatch(w, f, v)) ? TH_FISSURE_WATCHED : TH_FISSURE_TILE}>
            <div className={TH_FISSURE_TOP}>
              <span className="text-[11px] font-bold shrink-0 w-[52px]" style={{ color: TIER_COLOR[f.tier] ?? "#ccc" }}>{f.tier}</span>
              <span className={TH_FISSURE_CD}>{cd(f.expiry)}</span>
            </div>
            <div className={TH_FISSURE_MISSION}>{f.missionType}</div>
            <div className={TH_FISSURE_BOTTOM}>
              {f.enemy && <span className={TH_FISSURE_ENEMY}>{f.enemy}</span>}
              <span className={TH_FISSURE_NODE}>{f.node}</span>
            </div>
          </div>
        );

        return (
          <>
            <div className={TH_GROUP_FISSURES}>
              <span>Void Fissures</span>
              <div className={TH_FISSURE_TABS}>
                <button className={fissureTab === "normal" ? TH_TAB_ACTIVE : TH_TAB_BASE} onClick={() => setFissureTab("normal")}>Normal</button>
                <button className={fissureTab === "hard"   ? TH_TAB_ACTIVE : TH_TAB_BASE} onClick={() => setFissureTab("hard")}>Steel Path</button>
                <button className={fissureTab === "storm"  ? TH_TAB_ACTIVE : TH_TAB_BASE} onClick={() => setFissureTab("storm")}>Storms</button>
              </div>
              <button className={showWatchForm ? TH_WATCH_BTN_ACTIVE : TH_WATCH_BTN} onClick={() => setShowWatchForm(v => !v)} title="Manage fissure watches">
                <svg className={TH_WATCH_SVG} viewBox="0 0 16 16" fill="none"><path d="M2 4h12M4 8h8M6 12h4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/></svg>
              </button>
            </div>

            {showWatchForm && (
              <div className={TH_WATCH_PANEL}>
                {fissureWatches.length > 0 && (
                  <div className={TH_WATCH_CHIPS}>
                    {fissureWatches.map(w => (
                      <div key={w.id} className={TH_WATCH_CHIP}>
                        <span className={TH_CHIP_TIER} style={{ color: TIER_COLOR[w.tier] ?? "var(--text)" }}>{w.tier}</span>
                        {w.missionType !== "Any" && <span className={TH_CHIP_MT}>{w.missionType}</span>}
                        {w.variant !== "any" && <span className={TH_CHIP_VAR}>{VARIANTS.find(v => v.key === w.variant)?.label}</span>}
                        <button className={TH_CHIP_DEL} onClick={() => onRemoveWatch(w.id)}>×</button>
                      </div>
                    ))}
                  </div>
                )}
                <div className={TH_WATCH_FORM}>
                  <div className={TH_FORM_ROW}>
                    <span className={TH_FORM_LABEL}>Mode</span>
                    <div className={TH_PILL_GROUP}>
                      {VARIANTS.map(v => (
                        <button key={v.key} className={wVariant === v.key ? TH_PILL_ACTIVE : TH_PILL} onClick={() => setVariant(v.key)}>{v.label}</button>
                      ))}
                    </div>
                  </div>
                  <div className={TH_FORM_ROW}>
                    <span className={TH_FORM_LABEL}>Tier</span>
                    <div className={TH_PILL_GROUP_WRAP}>
                      {TIERS.map(t => (
                        <button key={t} className={wTier === t ? TH_PILL_ACTIVE : TH_PILL}
                          style={wTier === t && TIER_COLOR[t] ? { borderColor: TIER_COLOR[t], color: TIER_COLOR[t] } : undefined}
                          onClick={() => setWTier(t)}>{t}</button>
                      ))}
                    </div>
                  </div>
                  <div className={TH_FORM_ROW}>
                    <span className={TH_FORM_LABEL}>Type</span>
                    <div className={TH_PILL_GROUP_WRAP}>
                      {MISSION_TYPES_BY_VARIANT[wVariant].map(m => (
                        <button key={m} className={wMission === m ? TH_PILL_ACTIVE : TH_PILL} onClick={() => setWMission(m)}>{m}</button>
                      ))}
                    </div>
                  </div>
                  <button className={TH_ADD_BTN} onClick={async () => {
                    onAddWatch({ id: `${Date.now()}`, tier: wTier, missionType: wMission, variant: wVariant });
                    setWTier("Any"); setWMission("Any"); setVariant("any");
                    // Prompt here rather than from the poll loop, so the OS dialog
                    // arrives while the user is thinking about fissure alerts.
                    if (fissureNotifications && !(await ensurePermission())) setPermissionDenied(true);
                  }}>+ Add Watch</button>
                  <label className={TH_NOTIFY}>
                    <input className={TH_NOTIFY_INPUT} type="checkbox" checked={fissureNotifications} onChange={async e => {
                      if (!e.target.checked) { onFissureNotificationsChange(false); return; }
                      // Turn the setting back off when the OS refuses, rather than
                      // leaving a ticked box promising alerts that cannot arrive.
                      const granted = await ensurePermission();
                      onFissureNotificationsChange(granted);
                      if (!granted) setPermissionDenied(true);
                    }} />
                    Notify me when a watched fissure appears
                  </label>
                  {permissionDenied && (
                    <div className={TH_NOTIFY_DENIED}>Notifications are blocked for FrameForge in your system settings.</div>
                  )}
                </div>
              </div>
            )}

            {list.length === 0
              ? <div className={TH_MSG}>No active fissures.</div>
              : <div className={TH_FISSURE_GRID}>
                  {list.map((f, i) => <FissureTile key={i} f={f} v={fissureTab === "hard" ? "hard" : fissureTab === "storm" ? "storm" : "normal"} />)}
                </div>
            }
          </>
        );
      })()}
    </div>
  );
}
