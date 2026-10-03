import { useState, useEffect, useMemo, useCallback, memo, startTransition, useRef, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import ItemImg from "./ItemImg";
import { HelpTip } from "./shared/HelpTip";
import { SecondaryButton } from "./shared/ui/ActionButton";
import { CategoryButton, CAT_COUNT, CAT_TOTAL } from "./shared/ui/CategoryButton";
import { ModalCloseButton } from "./shared/ui/ModalCloseButton";
import { EmptyMessage, FilterBar, FilterChip, FilterSeparator, FoundrySearch } from "./shared/ui/FilterControls";
import FilterPresets from "./shared/FilterPresets";
import { PREFERENCE_KEYS } from "./constants/preferences";
import { matchesSearchTerms, splitSearchTerms } from "./lib/search";
import { WARFRAME_WIKI_BASE } from "./constants/urls";
import { TAURI_COMMANDS } from "./constants/tauri";
import { useCatalog } from "./hooks/useCatalog";
import type { ArchonShard, CatalogItem, CraftingJob, InventoryItem, RecipeComponent, RecipeComponentStatus, RecipeMap, RelicDropMap } from "./types/items";
import type { FoundryFilters } from "./types/filters";
import type { FilterPresetModule, FilterPresetSettings } from "./types/filterPresets";
import type { ViewMode } from "./types/ui";
import { ViewToggle } from "./shared/ViewToggle";
import sentientIcon from "./assets/SentientFactionIcon.webp";
import formaIcon from "./assets/forma-icon.png";

interface Props {
  inventory: Record<string, InventoryItem>;
  refreshKey: number;
  crafting: CraftingJob[];
  colorblindMode?: boolean;
  subsummedWarframes?: Set<string>;
  tracked: string[];
  onTrackToggle: (id: string) => void;
  pageSize?: number;
  filters: FoundryFilters;
  onFiltersChange: Dispatch<SetStateAction<FoundryFilters>>;
  filterPresets: FilterPresetSettings;
  onFilterPresetsChange: Dispatch<SetStateAction<FilterPresetSettings>>;
  onOpenSettings: (module: FilterPresetModule) => void;
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function fmt(n: number) { return n.toLocaleString(); }

function collectNeeds(
  nodes: RecipeComponent[],
  multiplier: number,
  acc: Map<string, { name: string; needed: number }>
) {
  for (const node of mergeComponents(nodes)) {
    const resultCount = node.result_count ?? 1;
    const craftsNeeded = Math.ceil((node.count * multiplier) / resultCount);
    if (node.components.length === 0) {
      const prev = acc.get(node.unique_name);
      acc.set(node.unique_name, { name: node.name, needed: (prev?.needed ?? 0) + node.count * multiplier });
    } else {
      collectNeeds(node.components, craftsNeeded, acc);
      const prev = acc.get(node.unique_name);
      acc.set(node.unique_name, { name: node.name, needed: (prev?.needed ?? 0) + node.count * multiplier });
    }
  }
}

function compStatus(comp: RecipeComponent, inventory: Record<string, InventoryItem>): RecipeComponentStatus {
  if ((inventory[comp.unique_name]?.quantity ?? 0) >= (comp.count || 1)) return "part";
  const bpUnique = comp.components[0]?.unique_name;
  if (bpUnique && (inventory[bpUnique]?.quantity ?? 0) > 0) return "blueprint";
  return "none";
}

/** Merge recipe components that share the same unique_name, summing their counts.
 *  Some recipes (e.g. Akbolto needing 2x Bolto) list the same item twice. */
function mergeComponents(comps: RecipeComponent[]): RecipeComponent[] {
  const seen = new Map<string, RecipeComponent>();
  for (const c of comps) {
    const existing = seen.get(c.unique_name);
    if (existing) {
      seen.set(c.unique_name, { ...existing, count: existing.count + c.count });
    } else {
      seen.set(c.unique_name, { ...c });
    }
  }
  return [...seen.values()];
}

function isLichWeapon(item: CatalogItem): boolean {
  return item.name.startsWith("Kuva ") || item.name.startsWith("Tenet ");
}


const LEVELABLE_CATS = new Set(["Warframes", "Primary", "Secondary", "Melee", "Companions", "Archwing", "Operator Weapons"]);

/** Effective max rank for an item. WFCD only sets maxLevelCap for items above 30;
 *  levelable-category items without it default to 30. Non-levelable items return null. */
function effectiveMaxCap(item: CatalogItem): number | null {
  if (item.max_level_cap != null && item.max_level_cap > 0) return item.max_level_cap;
  return LEVELABLE_CATS.has(item.category) ? 30 : null;
}

// ─── Tailwind class constants (converted from App.css Foundry rules) ─────────

const FY_ROOT = "flex flex-1 overflow-hidden min-w-0 min-h-0";
const FY_SIDEBAR = "flex w-[160px] shrink-0 flex-col overflow-hidden border-r border-border min-h-0";
const FY_SIDEBAR_CAT = "text-[12px]! px-[10px]! min-w-0";
const FY_CAT_LABEL = "overflow-hidden text-ellipsis whitespace-nowrap min-w-0";
const FY_SEARCH_WRAP = "px-[8px] pt-[6px] pb-[4px] shrink-0";
const FY_MAIN = "flex flex-1 flex-col overflow-hidden border-r border-border min-w-0 min-h-0";

const FY_GRID_SHELL = "flex-1 min-h-0 overflow-y-auto overflow-x-hidden content-start";
const FY_GRID = `${FY_GRID_SHELL} grid gap-[6px] p-[8px] grid-cols-[repeat(auto-fill,minmax(min(200px,100%),1fr))]`;
const FY_GRID_ICONS = `${FY_GRID_SHELL} grid gap-[6px] p-[8px] grid-cols-[repeat(auto-fill,88px)]`;
const FY_GRID_TEXT = `${FY_GRID_SHELL} grid gap-[6px] p-[8px] grid-cols-[repeat(auto-fill,minmax(160px,1fr))]`;
const FY_GRID_LIST = "flex-1 min-h-0 overflow-y-auto overflow-x-hidden flex flex-col gap-[1px] py-[4px]";

function craftGridClass(view: ViewMode): string {
  if (view === "icons") return FY_GRID_ICONS;
  if (view === "list" || view === "list-compact") return FY_GRID_LIST;
  if (view === "text-cards") return FY_GRID_TEXT;
  return FY_GRID;
}

const FY_PAGINATION = "flex items-center justify-center gap-[10px] px-[8px] py-[10px]";
const FY_PG_LABEL = "min-w-[80px] text-center text-[12px] text-muted";

const FY_CARD_SHELL = "relative grid h-[168px] min-w-[200px] cursor-pointer rounded-[8px] border transition-colors grid-cols-[88px_1fr] grid-rows-[96px_24px_24px_24px]";
const FY_CARD = `${FY_CARD_SHELL} border-border bg-surface hover:border-[rgba(56,139,253,.5)] hover:z-[5]`;
const FY_CARD_OWNED = `${FY_CARD_SHELL} border-[rgba(240,192,64,.6)] bg-[rgba(240,192,64,.04)] hover:border-[rgba(56,139,253,.5)] hover:z-[5]`;
const FY_CARD_READY = `${FY_CARD_SHELL} border-[rgba(56,139,253,.55)] bg-[rgba(56,139,253,.04)] hover:border-[rgba(56,139,253,.5)] hover:z-[5]`;

const FY_CC_IMAGE = "col-start-1 row-start-1 relative overflow-hidden bg-[rgba(0,0,0,.15)] border-r border-border [&_img]:block! [&_img]:h-[96px]! [&_img]:w-[88px]! [&_img]:rounded-none! [&_img]:object-cover! [&_.img-fallback]:h-[96px]! [&_.img-fallback]:w-[88px]! [&_.img-fallback]:rounded-none!";
const FY_CC_STAR = "absolute top-[3px] left-[5px] z-[2] cursor-pointer border-0 bg-transparent p-0 text-[14px] leading-none text-[rgba(255,255,255,.45)] hover:text-[#f0c040]";
const FY_CC_STAR_TRACKED = "absolute top-[3px] left-[5px] z-[2] cursor-pointer border-0 bg-transparent p-0 text-[14px] leading-none text-[#f0c040]";
const FY_CC_WIKI = "absolute top-[3px] right-[4px] z-[2] cursor-pointer rounded-[3px] border border-[rgba(56,139,253,.4)] bg-[rgba(0,0,0,.5)] px-1 py-px text-[8px] font-bold text-[#6ea8fe] hover:bg-[rgba(56,139,253,.25)]";
const FY_CC_NAME = "absolute inset-x-0 bottom-0 z-[2] overflow-hidden text-ellipsis whitespace-nowrap bg-[rgba(0,0,0,.7)] px-1 py-[2px] text-center text-[9px] font-bold text-white";
const FY_CC_MR = "col-start-1 row-start-2 flex items-center justify-center overflow-hidden border-r border-border";
const FY_CC_SUBSUMED = "pointer-events-none box-content h-[16px] w-[16px] shrink-0 rounded-[3px] bg-[rgba(200,40,40,.25)] p-[2px] object-contain drop-shadow-[0_0_2px_rgba(0,0,0,.8)]";
const FY_CC_BADGES = "col-start-1 row-start-3 flex items-center justify-center gap-[3px] overflow-hidden border-r border-r-border border-t border-t-[rgba(48,54,61,.4)] px-[3px]";
const FY_CC_TAGS = "col-start-1 row-start-4 flex flex-nowrap items-center justify-center gap-[2px] overflow-hidden border-r border-r-border border-t border-t-[rgba(48,54,61,.4)] px-[2px]";
const FY_CC_ING = "col-start-2 row-start-1 row-span-4 flex flex-col overflow-hidden";

const FY_COMP_SHELL = "flex h-[21px] min-h-0 min-w-0 items-center justify-between rounded-none border-b border-b-[rgba(48,54,61,.3)] px-[5px] flex-[1_1_0]";
const FY_COMP_ROW: Record<RecipeComponentStatus, string> = {
  none: `${FY_COMP_SHELL} bg-[rgba(255,255,255,.02)]`,
  blueprint: `${FY_COMP_SHELL} bg-[rgba(56,139,253,.12)]`,
  part: `${FY_COMP_SHELL} bg-[rgba(240,192,64,.12)]`,
};
const FY_COMP_NAME_BASE = "min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[11px]";
const FY_COMP_NAME: Record<RecipeComponentStatus, string> = {
  none: `${FY_COMP_NAME_BASE} text-muted`,
  blueprint: `${FY_COMP_NAME_BASE} text-[#6eb4ff]`,
  part: `${FY_COMP_NAME_BASE} text-[#f0c040]`,
};
const FY_COMP_BADGE = "ml-[3px] shrink-0 text-[9px] font-bold";
const FY_COMP_LOADING = "flex flex-1 items-center px-[6px] text-[10px] text-muted";
const FY_COMP_ACQUIRED = "flex flex-1 items-center justify-center px-[6px] text-center text-[10px] italic text-accent opacity-[0.85]";

const FY_MR_REQ = "whitespace-nowrap rounded-[3px] bg-[rgba(255,255,255,.07)] px-1 py-px text-[9px] font-bold text-[#8b949e]";
const FY_ROW_MR = "shrink-0 whitespace-nowrap rounded-[3px] bg-[rgba(255,255,255,.07)] px-1 py-px text-[10px] font-bold text-[#8b949e]";

const FY_ICON_SHELL = "relative flex h-[88px] w-[88px] cursor-pointer items-center justify-center overflow-hidden rounded-[8px] border bg-surface transition-colors [&_img]:size-[80px]! [&_img]:object-cover!";
const FY_ICON_CARD = `${FY_ICON_SHELL} border-border hover:border-[rgba(56,139,253,.5)]`;
const FY_ICON_CARD_OWNED = `${FY_ICON_SHELL} border-[rgba(240,192,64,.6)]`;
const FY_ICON_CARD_READY = `${FY_ICON_SHELL} border-[rgba(56,139,253,.55)]`;
const FY_ICON_BADGE = "absolute bottom-[2px] right-[3px] rounded-[3px] px-[3px] py-px text-[9px] font-bold";
const FY_ICON_BADGE_OWNED = `${FY_ICON_BADGE} bg-[rgba(240,192,64,.2)] text-[#f0c040]`;
const FY_ICON_BADGE_READY = `${FY_ICON_BADGE} bg-[rgba(56,139,253,.2)] text-accent`;

const FY_ROW_SHELL = "flex min-h-[34px] cursor-pointer items-center gap-[8px] border-b border-b-[rgba(48,54,61,.35)] px-[12px] py-[5px] transition-colors duration-100 hover:bg-[rgba(255,255,255,.03)]";
const FY_ROW_OWNED = `${FY_ROW_SHELL} border-l-2 border-l-[rgba(240,192,64,.7)]`;
const FY_ROW_READY = `${FY_ROW_SHELL} border-l-2 border-l-[rgba(56,139,253,.7)]`;
const FY_ROW_ICON = "flex h-[26px] w-[26px] shrink-0 items-center justify-center [&_img]:size-6! [&_img]:rounded-[3px]! [&_img]:object-cover!";
const FY_ROW_NAME = "min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[12px] font-medium text-foreground";
const FY_ROW_STATUS = "flex shrink-0 gap-[3px]";
const FY_ROW_PARTS = "shrink-0 text-[10px] text-muted";
const FY_ROW_ACQUIRED = "shrink-0 cursor-help text-[10px] italic text-muted";

const FY_TEXT_SHELL = "flex min-h-[70px] cursor-pointer flex-col gap-[4px] rounded-[8px] border bg-surface px-[10px] py-[8px] transition-colors";
const FY_TEXT_CARD = `${FY_TEXT_SHELL} border-border hover:border-[rgba(56,139,253,.5)]`;
const FY_TEXT_CARD_OWNED = `${FY_TEXT_SHELL} border-[rgba(240,192,64,.6)] bg-[rgba(240,192,64,.04)]`;
const FY_TEXT_CARD_READY = `${FY_TEXT_SHELL} border-[rgba(56,139,253,.55)] bg-[rgba(56,139,253,.04)]`;
const FY_CTC_NAME = "text-[12px] font-semibold leading-[1.3] text-foreground";
const FY_CTC_META = "flex flex-wrap items-center gap-[4px]";
const FY_CTC_TAGS = "flex flex-wrap gap-[3px]";
const FY_VAULT_BADGE = "whitespace-nowrap rounded-[3px] px-1 py-px text-[9px] font-bold tracking-[.02em]";
const FY_VAULT_YES = `${FY_VAULT_BADGE} border border-[rgba(255,107,107,.35)] bg-[rgba(255,107,107,.15)] text-[#ff6b6b]`;
const FY_VAULT_NO = `${FY_VAULT_BADGE} border border-[rgba(78,205,196,.3)] bg-[rgba(78,205,196,.12)] text-[#4ecdc4]`;
const FY_RELIC_ICON_WRAP = "relic-icon-wrap relative mr-[2px] flex shrink-0 cursor-help";
const FY_RELIC_ICON = "shrink-0 opacity-90";

const FY_TAG = "inline-flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[3px] text-[10px] font-bold cursor-default";
const FY_TAG_MASTERED = `${FY_TAG} bg-[rgba(78,205,196,.15)] text-[#4ecdc4]`;
const FY_TAG_OWNED = `${FY_TAG} bg-[rgba(240,192,64,.12)] text-[#f0c040]`;
const FY_TAG_READY = `${FY_TAG} bg-[rgba(56,139,253,.12)] text-accent`;
const FY_TAG_FOUNDRY = `${FY_TAG} bg-[rgba(224,123,0,.15)] text-[#e07b00]`;
const FY_TAG_KUVA = `${FY_TAG} bg-[rgba(157,108,255,.15)] text-[#9d6cff]`;
const FY_TAG_RANK = "inline-flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[3px] text-[8px] font-bold cursor-default bg-[rgba(255,255,255,.07)] text-muted";
const FY_TAG_ARCHON = "inline-flex h-[18px] w-auto shrink-0 items-center justify-center rounded-[3px] p-0 text-[10px] font-bold cursor-help border border-[rgba(180,140,255,.4)] bg-[rgba(25,12,50,.7)]";
const FY_TAG_FORMA = "relative inline-flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-[3px] text-[10px] font-bold cursor-help";
const FY_FORMA_WRAP = "absolute inset-0 flex items-center justify-center";
const FY_FORMA_IMG = "h-full w-full mix-blend-screen opacity-90";
const FY_FORMA_COUNT = "pointer-events-none absolute inset-0 flex items-center justify-center text-[9px] font-extrabold leading-none text-white [text-shadow:0_0_4px_#000,0_0_2px_#000]";
const FY_CB_BADGE = "shrink-0 rounded-[3px] px-1 py-px text-[9px] font-black leading-[1.4]";
const FY_CB_OWNED = `${FY_CB_BADGE} border border-[rgba(240,192,64,.4)] bg-[rgba(240,192,64,.2)] text-[#f0c040]`;
const FY_CB_READY = `${FY_CB_BADGE} border border-[rgba(56,139,253,.4)] bg-[rgba(56,139,253,.2)] text-[#6ea8fe]`;

const FY_MODAL_OVERLAY = "fixed inset-0 z-[200] flex items-center justify-center bg-[rgba(0,0,0,.72)] p-[20px]";
const FY_MODAL = "flex max-h-[calc(82vh_/_var(--ff-scale,1))] w-[min(680px,95vw)] flex-col overflow-hidden rounded-[12px] border border-border bg-surface shadow-[0_20px_60px_rgba(0,0,0,.6)]";
const FY_MODAL_HEADER = "flex shrink-0 items-center gap-[10px] border-b border-border px-[18px] py-[14px]";
const FY_MODAL_TITLE = "min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[15px] font-semibold";
const FY_MODAL_BADGE = "shrink-0 rounded-[4px] border border-[rgba(224,123,0,.3)] bg-[rgba(224,123,0,.15)] px-[7px] py-[2px] text-[11px] text-[#e07b00]";
const FY_MODAL_TABS = "flex shrink-0 gap-0 border-b border-border px-[18px] pt-[8px]";
const FY_MODE_TOGGLE = "px-[10px] py-[3px] rounded-[6px] border cursor-pointer text-[11px] transition-all duration-150";
const FY_MODE_TOGGLE_ON = "border-accent! text-accent! bg-[rgba(56,139,253,.1)]!";
const FY_MODE_TOGGLE_OFF = "border-border bg-transparent text-muted hover:border-accent hover:text-accent";
const FY_MODAL_BODY = "flex-1 overflow-y-auto px-[18px] py-[12px]";
const FY_KUVA_NOTICE = "flex items-start gap-[12px] rounded-[8px] border border-[rgba(157,108,255,.25)] bg-[rgba(157,108,255,.08)] p-[16px] text-[13px] leading-[1.6] text-foreground";
const FY_KUVA_ICON = "shrink-0 text-[20px]";
const FY_TRACK_BTN = "shrink-0 cursor-pointer whitespace-nowrap rounded-[6px] border border-border bg-transparent px-[8px] py-[2px] text-[11px] text-muted transition-colors duration-150 hover:border-[#f0c040] hover:text-[#f0c040]";
const FY_TRACK_BTN_ON = "shrink-0 cursor-pointer whitespace-nowrap rounded-[6px] border border-[#f0c040] bg-[rgba(240,192,64,.1)] px-[8px] py-[2px] text-[11px] text-[#f0c040] transition-colors duration-150";

const FY_NEEDS_LIST = "px-[8px] py-[4px]";
const FY_NEEDS_ROW = "flex items-center justify-between gap-[12px] border-b border-b-[rgba(48,54,61,.5)] px-[8px] py-[6px] text-[13px]";
const FY_NEEDS_NAME = "min-w-0 flex-1 text-foreground";
const FY_NEEDS_COUNTS = "flex shrink-0 items-center gap-[4px] tabular-nums";
const FY_QTY_HAVE = "text-success";
const FY_QTY_NEED = "text-danger";
const FY_QTY_SEP = "text-muted";
const FY_QTY_REQUIRED = "text-muted";
const FY_SHORTAGE = "shrink-0 rounded-[4px] bg-[rgba(248,81,73,.12)] px-[5px] py-px text-[11px] font-semibold text-danger";
const FY_REC_ROW = "mx-[8px] my-px flex items-center gap-[6px] rounded-[4px] px-[12px] py-[5px] text-[13px] transition-colors duration-100 hover:bg-[rgba(255,255,255,.04)]";
const FY_CHEVRON = "w-[12px] shrink-0 text-[11px] text-muted";
const FY_CHEVRON_LEAF = "w-[12px] shrink-0 text-[11px] text-border";
const FY_REC_NAME = "min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-foreground";
const FY_REC_COUNTS = "flex shrink-0 items-center gap-[2px] tabular-nums text-[12px]";

// ─── Relic helpers ────────────────────────────────────────────────────────────

// Pentagon where each of the 5 sides represents one shard slot.
// Filled slot → side drawn in shard color; empty slot → dim grey.
function ArchonCrystalIcon({ shards }: { shards: ArchonShard[] }) {
  try {
    const hasAny = shards && shards.length > 0;
    const lines = hasAny
      ? shards.map(s => `${s.tauforged ? "✦ " : ""}${s.type}${s.boost ? ` · ${s.boost}` : ""}`).join("\n")
      : "No Archon Shards";

    const R = 6.5;
    const cx = 8, cy = 8.2; // slight down-shift so top vertex sits at ~y=1.7
    // 5 vertices starting from the top (270°), going clockwise
    const verts = Array.from({ length: 5 }, (_, i) => {
      const a = (Math.PI / 180) * (270 + i * 72);
      return { x: cx + R * Math.cos(a), y: cy + R * Math.sin(a) };
    });

    const pts = verts.map(v => `${v.x.toFixed(2)},${v.y.toFixed(2)}`).join(" ");
    return (
      <span
        className={FY_TAG_ARCHON}
        title={`Archon Shards:\n${lines}`}
      >
        <svg width="18" height="18" viewBox="0 0 16 16" fill="none" className="block">
          {/* 1. Tauforged wedge fills — drawn first so outline goes on top */}
          {Array.from({ length: 5 }, (_, i) => {
            const s = shards?.[i];
            if (!s?.tauforged) return null;
            const a = verts[i], b = verts[(i + 1) % 5];
            const wpts = `${a.x.toFixed(2)},${a.y.toFixed(2)} ${b.x.toFixed(2)},${b.y.toFixed(2)} ${cx.toFixed(2)},${cy.toFixed(2)}`;
            return <polygon key={i} points={wpts} fill={s.color} fillOpacity={0.8} />;
          })}
          {/* 2. Dividing lines from each vertex to center */}
          {verts.map((v, i) => (
            <line key={`div-${i}`}
              x1={v.x.toFixed(2)} y1={v.y.toFixed(2)}
              x2={cx.toFixed(2)} y2={cy.toFixed(2)}
              stroke="rgba(0,0,0,0.45)" strokeWidth={0.6}
            />
          ))}
          {/* 3. Normal shard edge lines */}
          {Array.from({ length: 5 }, (_, i) => {
            const s = shards?.[i];
            if (!s || s.tauforged) return null;
            const a = verts[i], b = verts[(i + 1) % 5];
            return (
              <line key={`edge-${i}`}
                x1={a.x.toFixed(2)} y1={a.y.toFixed(2)}
                x2={b.x.toFixed(2)} y2={b.y.toFixed(2)}
                stroke={s.color} strokeWidth={2.4} strokeLinecap="butt"
              />
            );
          })}
          {/* 4. Outer pentagon outline drawn last — always on top, always one clean connected shape */}
          <polygon points={pts} fill="none" stroke="rgba(255,255,255,0.45)" strokeWidth={0.8} />
        </svg>
      </span>
    );
  } catch {
    return null;
  }
}


function RelicIcon() {
  return (
    <svg viewBox="0 0 20 26" width="11" height="14" fill="none" xmlns="http://www.w3.org/2000/svg" className={FY_RELIC_ICON}>
      <ellipse cx="10" cy="13" rx="8.5" ry="11.5" fill="rgba(255,220,100,.15)" stroke="rgba(255,220,100,.7)" strokeWidth="1.2"/>
      <path d="M10 4 C7 7 6 10 8 13 C10 16 9 19 10 22" stroke="rgba(255,220,100,.9)" strokeWidth="1.3" strokeLinecap="round" fill="none"/>
      <path d="M10 4 C13 7 14 10 12 13 C10 16 11 19 10 22" stroke="rgba(255,220,100,.6)" strokeWidth="0.9" strokeLinecap="round" fill="none"/>
    </svg>
  );
}

function FormaIcon({ count }: { count: number }) {
  return (
    <span className={FY_TAG_FORMA} title={`${count} Forma applied`}>
      <span className={FY_FORMA_WRAP}>
        <img src={formaIcon} alt="" className={FY_FORMA_IMG} />
      </span>
      <span className={FY_FORMA_COUNT}>{count}</span>
    </span>
  );
}

const RELIC_SUFFIXES = ["Bronze", "Silver", "Gold", "Platinum"];
function ownsRelicVariant(relicUnique: string, inventory: Record<string, InventoryItem>): boolean {
  const base = relicUnique.replace(/(Bronze|Silver|Gold|Platinum)$/, "");
  return RELIC_SUFFIXES.some(s => (inventory[`${base}${s}`]?.quantity ?? 0) > 0);
}

// ─── Comp row (used inside modal tree) ───────────────────────────────────────

function CompRow({ comp, inventory, relicDrops, relicNames }: {
  comp: RecipeComponent; inventory: Record<string, InventoryItem>;
  relicDrops: RelicDropMap; relicNames: Record<string, string>;
}) {
  const status = compStatus(comp, inventory);
  const ownedRelics = [...new Set(
    (relicDrops[comp.unique_name] ?? [])
      .filter(r => ownsRelicVariant(r, inventory))
      .map(r => {
        const base = r.replace(/(Bronze|Silver|Gold|Platinum)$/, "");
        const owned = RELIC_SUFFIXES.find(s => (inventory[`${base}${s}`]?.quantity ?? 0) > 0);
        const key = owned ? `${base}${owned}` : r;
        return relicNames[key] ?? relicNames[r] ?? r.split("/").pop() ?? r;
      })
  )];
  return (
    <div className={FY_COMP_ROW[status]}>
      {ownedRelics.length > 0 && (
        <span className={FY_RELIC_ICON_WRAP} title={ownedRelics.join("\n")}><RelicIcon /></span>
      )}
      <span className={FY_COMP_NAME[status]}>{comp.name}</span>
      {status === "part"      && <span className={`${FY_COMP_BADGE} text-[#f0c040]`}>✓</span>}
      {status === "blueprint" && <span className={`${FY_COMP_BADGE} text-accent`}>BP</span>}
    </div>
  );
}

// ─── Tree node (modal recipe tree) ───────────────────────────────────────────

function TreeNode({ node, inventory, depth }: {
  node: RecipeComponent; inventory: Record<string, InventoryItem>; depth: number;
}) {
  const owned = inventory[node.unique_name]?.quantity ?? 0;
  const enough = owned >= node.count;
  const hasChildren = node.components.length > 0;
  // Don't auto-expand satisfied nodes — hides unnecessary sub-trees (e.g. Control Module Blueprint when you have 443)
  const [open, setOpen] = useState(!enough && depth < 3);
  return (
    <div style={{ marginLeft: depth * 16 }}>
      <div
        className={`${FY_REC_ROW}${enough ? " opacity-70" : ""} ${hasChildren ? "cursor-pointer" : "cursor-default"}`}
        onClick={() => hasChildren && setOpen(o => !o)}
      >
        {hasChildren
          ? <span className={FY_CHEVRON}>{open ? "▾" : "▸"}</span>
          : <span className={FY_CHEVRON_LEAF}>·</span>}
        <span className={FY_REC_NAME}>{node.name}</span>
        <span className={FY_REC_COUNTS}>
          <span className={enough ? FY_QTY_HAVE : FY_QTY_NEED}>{fmt(owned)}</span>
          <span className={FY_QTY_SEP}>/</span>
          <span className={FY_QTY_REQUIRED}>{fmt(node.count)}</span>
        </span>
        {!enough && <span className={FY_SHORTAGE}>−{fmt(node.count - owned)}</span>}
      </div>
      {hasChildren && open && mergeComponents(node.components).map((child, i) => (
        <TreeNode key={i} node={child} inventory={inventory} depth={depth + 1} />
      ))}
    </div>
  );
}

// ─── Recipe modal ─────────────────────────────────────────────────────────────

function RecipeModal({ item, recipe, inventory, isTracked, onTrack, onClose, crafting }: {
  item: CatalogItem; recipe: RecipeComponent[] | null;
  inventory: Record<string, InventoryItem>; isTracked: boolean;
  onTrack: () => void; onClose: () => void; crafting: CraftingJob[];
}) {
  const [mode, setMode] = useState<"tree" | "needs">("tree");
  const isKuva     = isLichWeapon(item);
  const isAcquired = !!item.source_type;
  const craftJob = crafting.find(c =>
    c.unique_name === item.unique_name ||
    (recipe && recipe.length > 0 && recipe[0].unique_name === c.unique_name)
  );

  const needs = useMemo(() => {
    if (!recipe?.length) return [];
    const acc = new Map<string, { name: string; needed: number }>();
    collectNeeds(recipe, 1, acc);
    return Array.from(acc.entries())
      .map(([unique_name, { name, needed }]) => ({
        unique_name, name, needed, owned: inventory[unique_name]?.quantity ?? 0,
      }))
      .filter(r => r.owned < r.needed)
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [recipe, inventory]);

  return (
    <div className={FY_MODAL_OVERLAY} onClick={onClose}>
      <div className={FY_MODAL} onClick={e => e.stopPropagation()}>

        {/* Header */}
        <div className={FY_MODAL_HEADER}>
          <ItemImg imageName={item.image_name} category={item.category} size={36} />
          <span className={FY_MODAL_TITLE}>{item.name}</span>
          {craftJob && <span className={FY_MODAL_BADGE} title={`Building — ${item.name}`}>⚒ Building</span>}
          <button className={isTracked ? FY_TRACK_BTN_ON : FY_TRACK_BTN} onClick={onTrack}>
            {isTracked ? "★ Tracked" : "☆ Track"}
          </button>
          <ModalCloseButton onClick={onClose}>✕</ModalCloseButton>
        </div>

        {isKuva ? (
          <div className={FY_MODAL_BODY}>
            <div className={FY_KUVA_NOTICE}>
              <span className={FY_KUVA_ICON}>🔱</span>
              <div>
                <strong>{item.name}</strong> is obtained by converting a{" "}
                {item.name.startsWith("Kuva ") ? <strong>Kuva Lich</strong> : <strong>Tenet Sister</strong>},
                not crafted from a Blueprint.
              </div>
            </div>
          </div>
        ) : isAcquired ? (
          <div className={FY_MODAL_BODY}>
            <div className={FY_KUVA_NOTICE}>
              <span className={FY_KUVA_ICON}>🎮</span>
              <div>
                <strong>{item.name}</strong> is acquired in-game and cannot be crafted in the Foundry.
              </div>
            </div>
          </div>
        ) : (
          <>
            <div className={FY_MODAL_TABS}>
              <button className={`${FY_MODE_TOGGLE} ${mode === "tree" ? FY_MODE_TOGGLE_ON : FY_MODE_TOGGLE_OFF}`} onClick={() => setMode("tree")}>Full tree</button>
              <button className={`${FY_MODE_TOGGLE} ${mode === "needs" ? FY_MODE_TOGGLE_ON : FY_MODE_TOGGLE_OFF}`} onClick={() => setMode("needs")}>What I need</button>
            </div>
            <div className={FY_MODAL_BODY}>
              {!recipe ? (
                <EmptyMessage>Loading…</EmptyMessage>
              ) : recipe.length === 0 ? (
                <EmptyMessage>No recipe data.</EmptyMessage>
              ) : mode === "tree" ? (
                mergeComponents(recipe).map((node, i) => <TreeNode key={i} node={node} inventory={inventory} depth={0} />)
              ) : needs.length === 0 ? (
                <EmptyMessage>✓ You have everything needed.</EmptyMessage>
              ) : (
                <div className={FY_NEEDS_LIST}>
                  {needs.map(r => (
                    <div key={r.unique_name} className={FY_NEEDS_ROW}>
                      <span className={FY_NEEDS_NAME}>{r.name}</span>
                      <span className={FY_NEEDS_COUNTS}>
                        <span className={FY_QTY_NEED}>{fmt(r.owned)}</span>
                        <span className={FY_QTY_SEP}>/</span>
                        <span className={FY_QTY_REQUIRED}>{fmt(r.needed)}</span>
                        <span className={FY_SHORTAGE}>−{fmt(r.needed - r.owned)}</span>
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

// ─── Craft card ───────────────────────────────────────────────────────────────

const CraftCard = memo(function CraftCard({ item, recipe, inventory, relicDrops, relicNames, crafting, isTracked, onTrack, onOpen, subsummedWarframes, view }: {
  item: CatalogItem; recipe: RecipeComponent[] | null;
  inventory: Record<string, InventoryItem>; relicDrops: RelicDropMap;
  relicNames: Record<string, string>;
  crafting: CraftingJob[]; isTracked: boolean;
  onTrack: (item: CatalogItem) => void;
  onOpen: (item: CatalogItem) => void;
  subsummedWarframes: Set<string>;
  view: ViewMode;
}) {
  const invEntry   = inventory[item.unique_name];
  const isOwned    = (invEntry?.quantity ?? 0) > 0;
  const rank       = invEntry?.mastery_rank;
  const effCap     = effectiveMaxCap(item);
  const isMastered = rank != null && effCap != null && rank >= effCap;
  const isSubsumed  = item.category === "Warframes" && subsummedWarframes.has(item.unique_name);
  const shards      = item.category === "Warframes" ? (invEntry?.archon_shards ?? []) : [];
  const formaCount  = invEntry?.forma_count ?? 0;
  // Memory scanner stores the recipe/blueprint path; catalog uses the result-item path.
  // Check both so items like Forma (recipe path ≠ item path) still get the badge.
  const isCrafting = crafting.some(c =>
    c.unique_name === item.unique_name ||
    (recipe && recipe.length > 0 && recipe[0].unique_name === c.unique_name)
  );
  const isKuva     = isLichWeapon(item);
  const mergedRecipe = recipe ? mergeComponents(recipe) : recipe;
  // ⚡ Ready = you have every ingredient itself (not just its blueprint).
  const allParts = mergedRecipe && mergedRecipe.length > 0 && mergedRecipe.every(c =>
    (inventory[c.unique_name]?.quantity ?? 0) >= (c.count || 1)
  );

  if (view === "icons") {
    return (
      <div className={isOwned ? FY_ICON_CARD_OWNED : allParts ? FY_ICON_CARD_READY : FY_ICON_CARD}
        title={`${item.name}${isOwned ? " (owned)" : allParts ? " (ready)" : ""}`}
        onClick={() => onOpen(item)}>
        <ItemImg imageName={item.image_name} category={item.category} size={72} />
        {isOwned && <span className={FY_ICON_BADGE_OWNED}>✓✓</span>}
        {!isOwned && allParts && <span className={FY_ICON_BADGE_READY}>⚡</span>}
      </div>
    );
  }

  if (view === "list" || view === "list-compact") {
    return (
      <div
        className={isOwned ? FY_ROW_OWNED : allParts ? FY_ROW_READY : FY_ROW_SHELL}
        onClick={() => onOpen(item)}>
        {view === "list" && (
          <div className={FY_ROW_ICON}>
            <ItemImg imageName={item.image_name} category={item.category} size={24} />
          </div>
        )}
        <div className={FY_ROW_NAME}>{item.name}</div>
        {item.mastery_req != null && item.mastery_req > 0 &&
          <span className={FY_ROW_MR}>MR {item.mastery_req}</span>}
        <div className={FY_ROW_STATUS}>
          {isMastered && <span className={FY_TAG_MASTERED} title="Mastered">★</span>}
          {isOwned && !isMastered && <span className={FY_TAG_OWNED}>✓✓</span>}
          {!isOwned && allParts && <span className={FY_TAG_READY}>⚡</span>}
          {isCrafting && <span className={FY_TAG_FOUNDRY} title="Building">⚒</span>}
          {formaCount > 0 && <FormaIcon count={formaCount} />}
        </div>
        {item.source_type
          ? <span className={FY_ROW_ACQUIRED}>Acquired in-game</span>
          : mergedRecipe && mergedRecipe.length > 0
            ? <span className={FY_ROW_PARTS}>{mergedRecipe.length} part{mergedRecipe.length !== 1 ? "s" : ""}</span>
            : null}
      </div>
    );
  }

  if (view === "text-cards") {
    return (
      <div
        className={isOwned ? FY_TEXT_CARD_OWNED : allParts ? FY_TEXT_CARD_READY : FY_TEXT_CARD}
        onClick={() => onOpen(item)}>
        <div className={FY_CTC_NAME}>{item.name}</div>
        <div className={FY_CTC_META}>
          {item.vaulted === true  && <span className={FY_VAULT_YES}>🔒 Vaulted</span>}
          {item.vaulted === false && <span className={FY_VAULT_NO}>🔓 Unvaulted</span>}
          {item.mastery_req != null && item.mastery_req > 0 &&
            <span className={FY_MR_REQ}>MR {item.mastery_req}</span>}
        </div>
        <div className={FY_CTC_TAGS}>
          {isMastered && <span className={FY_TAG_MASTERED} title="Mastered">★</span>}
          {isOwned && !isMastered && <span className={FY_TAG_OWNED}>✓✓</span>}
          {!isOwned && allParts && <span className={FY_TAG_READY}>⚡</span>}
          {isCrafting && <span className={FY_TAG_FOUNDRY} title="Building">⚒</span>}
          {formaCount > 0 && <FormaIcon count={formaCount} />}
        </div>
      </div>
    );
  }

  return (
    <div
      className={isOwned ? FY_CARD_OWNED : allParts ? FY_CARD_READY : FY_CARD}
      onClick={() => onOpen(item)}
    >
      {/* Col 1, rows 1-4: image block with star/wiki/name overlaid */}
      <div className={FY_CC_IMAGE}>
        <ItemImg imageName={item.image_name} category={item.category} size={78} />
        <button className={isTracked ? FY_CC_STAR_TRACKED : FY_CC_STAR}
          onClick={e => { e.stopPropagation(); onTrack(item); }}>{isTracked ? "★" : "☆"}</button>
        <button className={FY_CC_WIKI}
          onClick={e => { e.stopPropagation(); invoke(TAURI_COMMANDS.OPEN_URL, { url:`${WARFRAME_WIKI_BASE}/${item.name.replace(" Blueprint","").replace(/\s+/g,"_")}` }).catch(()=>{}); }}>wiki</button>
        <span className={FY_CC_NAME}>{item.name}</span>
      </div>

      {/* Col 1, row 5: MR requirement + subsumed indicator */}
      <div className={FY_CC_MR}>
        {isSubsumed && <img src={sentientIcon} className={FY_CC_SUBSUMED} title="Subsumed into Helminth" alt="Subsumed" />}
        {item.mastery_req != null && item.mastery_req > 0 &&
          <span className={FY_MR_REQ}>MR {item.mastery_req}</span>}
      </div>

      {/* Col 1, row 6: vault / kuva / acquired badges */}
      <div className={FY_CC_BADGES}>
        {item.vaulted === true  && <span className={FY_VAULT_YES}>🔒 Vaulted</span>}
        {item.vaulted === false && <span className={FY_VAULT_NO}>🔓 Unvaulted</span>}
        {isKuva && <span className={FY_TAG_KUVA} title="Lich/Sister">🔱</span>}
      </div>

      {/* Col 1, row 7: status tags */}
      <div className={FY_CC_TAGS}>
        {isMastered && <span className={FY_TAG_MASTERED} title="Mastered">★</span>}
        {isOwned && !isMastered && rank != null && <span className={FY_TAG_RANK}>R{rank}</span>}
        {isCrafting  && <span className={FY_TAG_FOUNDRY} title="Building">⚒</span>}
        {formaCount > 0 && <FormaIcon count={formaCount} />}
        {shards.length > 0 && <ArchonCrystalIcon shards={shards} />}
        {isOwned     && <span className={FY_CB_OWNED}>✓✓</span>}
        {!isOwned && allParts && <span className={FY_CB_READY}>⚡</span>}
      </div>

      {/* Col 2, rows 1-7: ingredient list — rows grow to fill available height */}
      <div className={FY_CC_ING}>
        {recipe === null ? (
          <div className={FY_COMP_LOADING}>Loading…</div>
        ) : item.source_type ? (
          <div className={FY_COMP_ACQUIRED}>Acquired in-game</div>
        ) : recipe.length === 0 ? (
          <div className={FY_COMP_LOADING}>No recipe</div>
        ) : (
          mergedRecipe!.map((comp, i) => (
            <CompRow key={i} comp={comp} inventory={inventory} relicDrops={relicDrops} relicNames={relicNames} />
          ))
        )}
      </div>
    </div>
  );
}, (prev, next) => {
  // Only re-render when props that affect this card's display actually change.
  // Avoids re-rendering all cards on every 10-second inventory scan.
  if (prev.view          !== next.view)          return false;
  if (prev.item          !== next.item)          return false;
  if (prev.recipe        !== next.recipe)        return false;
  if (prev.isTracked     !== next.isTracked)     return false;
  if (prev.crafting      !== next.crafting)      return false;
  if (prev.onTrack       !== next.onTrack)       return false;
  if (prev.onOpen        !== next.onOpen)        return false;
  if (prev.relicDrops    !== next.relicDrops)    return false;
  if (prev.relicNames    !== next.relicNames)    return false;
  if (prev.subsummedWarframes !== next.subsummedWarframes) return false;
  // Inventory: only check keys this specific card reads
  const keys = new Set<string>([prev.item.unique_name]);
  for (const c of (prev.recipe ?? [])) {
    keys.add(c.unique_name);
    if (c.components[0]) keys.add(c.components[0].unique_name);
  }
  for (const k of keys) {
    if ((prev.inventory[k]?.quantity    ?? 0)    !== (next.inventory[k]?.quantity    ?? 0))    return false;
    if ((prev.inventory[k]?.mastery_rank ?? null) !== (next.inventory[k]?.mastery_rank ?? null)) return false;
  }
  const pShards = prev.inventory[prev.item.unique_name]?.archon_shards;
  const nShards = next.inventory[next.item.unique_name]?.archon_shards;
  if ((pShards?.length ?? 0) !== (nShards?.length ?? 0)) return false;
  if ((prev.inventory[prev.item.unique_name]?.forma_count ?? 0) !== (next.inventory[next.item.unique_name]?.forma_count ?? 0)) return false;
  return true;
});

// ─── Foundry ─────────────────────────────────────────────────────────────────

const CRAFT_CATEGORIES = [
  "All", "Warframes", "Primary", "Secondary", "Melee",
  "Companions", "Archwing", "Operator Weapons", "Parts", "Blueprints", "Miscellaneous",
];

export default function Foundry({ inventory, refreshKey, crafting, subsummedWarframes = new Set(), tracked, onTrackToggle, pageSize = 30, filters, onFiltersChange, filterPresets, onFilterPresetsChange, onOpenSettings }: Props) {
  const { catalog, relicDropMap } = useCatalog();
  const [craftable, setCraftable] = useState<CatalogItem[]>([]);
  const [recipes, setRecipes]     = useState<Map<string, RecipeComponent[]>>(new Map());
  const [modalItem, setModalItem] = useState<CatalogItem | null>(null);
  const [inputSearch, setInputSearch] = useState(filters.search);
  const [page, setPage] = useState(0);
  const [craftView, setCraftView] = useState<ViewMode>(() =>
    (localStorage.getItem(PREFERENCE_KEYS.FOUNDRY_VIEW) as ViewMode | null) ?? "cards"
  );

  // Refs so debounce closure always reads latest values without stale captures
  const filtersRef = useRef(filters);
  filtersRef.current = filters;
  const onFiltersChangeRef = useRef(onFiltersChange);
  onFiltersChangeRef.current = onFiltersChange;

  const trackedSet = useMemo(() => new Set(tracked), [tracked]);

  // Sync local input when parent resets search through Clear filters.
  useEffect(() => { setInputSearch(filters.search); }, [filters.search]); // eslint-disable-line

  // Wait 150 ms after last keystroke before propagating to parent filters
  useEffect(() => {
    if (inputSearch === filtersRef.current.search) return;
    const id = setTimeout(() => {
      const f = filtersRef.current;
      onFiltersChangeRef.current({ ...f, search: inputSearch, ...(inputSearch ? { activeCat: "All" as any } : {}) });
    }, 150);
    return () => clearTimeout(id);
  }, [inputSearch]); // eslint-disable-line

  const { search, activeCat, filterPrime, filterNonPrime, filterVaulted, filterUnvaulted, filterMastered, filterUnmastered, filterOwned, filterUnowned, filterReady, filterLvlCap, ignoreFormaKuva } = filters;
  const set = <K extends keyof FoundryFilters>(k: K, v: FoundryFilters[K]) => onFiltersChange({ ...filters, [k]: v });

  useEffect(() => {
    let cancelled = false;
    invoke<CatalogItem[]>(TAURI_COMMANDS.GET_CRAFTABLE_ITEMS)
      .then(items => { if (!cancelled) setCraftable(items); })
      .catch(() => { if (!cancelled) setCraftable([]); });
    return () => { cancelled = true; };
  }, [refreshKey]);

  const relicDrops = useMemo(() => relicDropMap, [relicDropMap]);
  const relicNames = useMemo(() => {
    const map: Record<string, string> = {};
    for (const i of catalog) if (i.category === "Relics") map[i.unique_name] = i.name;
    return map;
  }, [catalog]);

  const visible = useMemo(() => {
    const searchTerms = splitSearchTerms(search);
    return craftable
      .filter(i => i.category === activeCat || activeCat === "All")
      .filter(i => matchesSearchTerms(searchTerms, i.name))
      .filter(i => !filterPrime    || i.name.includes("Prime") || i.vaulted != null)
      .filter(i => !filterNonPrime || (!i.name.includes("Prime") && i.vaulted == null))
      .filter(i => !filterVaulted   || i.vaulted === true)
      .filter(i => !filterUnvaulted || i.vaulted === false)
      .filter(i => {
        if (!filterMastered && !filterUnmastered) return true;
        if (!i.masterable) return false; // WFCD says not masterable → exclude from both filters
        const rank = inventory[i.unique_name]?.mastery_rank ?? 0;
        const cap = effectiveMaxCap(i) ?? 30;
        return filterMastered ? rank >= cap : rank < cap;
      })
      .filter(i => {
        if (!filterOwned && !filterUnowned) return true;
        if (ignoreFormaKuva && (i.name.includes("Forma") || i.name === "Kuva")) return filterOwned;
        const owned = (inventory[i.unique_name]?.quantity ?? 0) > 0;
        return filterOwned ? owned : !owned;
      })
      .filter(i => {
        if (!filterReady) return true;
        const r = recipes.get(i.unique_name);
        if (!r || r.length === 0) return false;
        if ((inventory[i.unique_name]?.quantity ?? 0) > 0) return false;
        return mergeComponents(r).every(c => (inventory[c.unique_name]?.quantity ?? 0) >= (c.count || 1));
      })
      .filter(i => !filterLvlCap || (i.max_level_cap != null && i.max_level_cap > 30));
  }, [craftable, activeCat, search, filterPrime, filterNonPrime, filterVaulted, filterUnvaulted,
      filterMastered, filterUnmastered, filterOwned, filterUnowned, filterReady, filterLvlCap, ignoreFormaKuva,
      // Only pull in inventory/recipes when a filter that actually reads them is active.
      // Without this guard, every 10-second scanner update re-renders all 100+ cards.
      (filterMastered || filterUnmastered || filterOwned || filterUnowned || filterReady) ? inventory : null,
      filterReady ? recipes : null,
  ]);

  // Reset to page 1 when the user changes a filter, search, or category —
  // but NOT when inventory or recipes update in the background.
  // Without this guard, every 10-second scan resets the page mid-browse.
  useEffect(() => { setPage(0); }, [ // eslint-disable-line
    activeCat, search, filterPrime, filterNonPrime, filterVaulted, filterUnvaulted,
    filterMastered, filterUnmastered, filterOwned, filterUnowned, filterReady, filterLvlCap, ignoreFormaKuva,
    craftable, pageSize,
  ]);
  const PAGE_SIZE = pageSize;
  const pageCount = Math.ceil(visible.length / PAGE_SIZE);
  const pagedItems = useMemo(() => visible.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE), [visible, page, PAGE_SIZE]);

  // Only fetch recipes for cards on the current page; the previous full-catalog
  // request retained every recipe while this page was hidden.
  useEffect(() => {
    const toLoad = pagedItems.filter(i => !recipes.has(i.unique_name));
    if (toLoad.length === 0) return;
    let cancelled = false;
    invoke<RecipeMap>(TAURI_COMMANDS.GET_RECIPES_BULK, {
      uniqueNames: toLoad.map(i => i.unique_name),
    }).then(result => {
      if (cancelled) return;
      startTransition(() => {
        setRecipes(prev => {
          const next = new Map(prev);
          for (const item of toLoad) {
            next.set(item.unique_name, result[item.unique_name] ?? []);
          }
          return next;
        });
      });
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [pagedItems, recipes]);

  // Load recipe for modal item
  useEffect(() => {
    if (!modalItem || recipes.has(modalItem.unique_name)) return;
    invoke<RecipeComponent[]>(TAURI_COMMANDS.GET_RECIPE, { uniqueName: modalItem.unique_name })
      .then(r => setRecipes(prev => new Map(prev).set(modalItem.unique_name, r ?? [])))
      .catch(() => {});
  }, [modalItem]);

  const handleTrack = useCallback((item: CatalogItem) => {
    onTrackToggle(item.unique_name);
  }, [onTrackToggle]);

  const handleOpen = useCallback((item: CatalogItem) => {
    setModalItem(item);
  }, []);

  const categoryCounts = useMemo(() => {
    const counts: Record<string, number> = { All: craftable.length };
    for (const i of craftable) counts[i.category] = (counts[i.category] ?? 0) + 1;
    return counts;
  }, [craftable]);

  const modalRecipe = modalItem ? (recipes.get(modalItem.unique_name) ?? null) : null;

  // Close modal on Escape
  useEffect(() => {
    if (!modalItem) return;
    const handler = (e: KeyboardEvent) => { if (e.key === "Escape") setModalItem(null); };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [modalItem]);

  return (
    <div className={FY_ROOT}>

      {/* ── Modal overlay ── */}
      {modalItem && (
        <RecipeModal
          item={modalItem}
          recipe={modalRecipe}
          inventory={inventory}
          isTracked={tracked.includes(modalItem.unique_name)}
          onTrack={() => handleTrack(modalItem)}
          onClose={() => setModalItem(null)}
          crafting={crafting}
        />
      )}

      {/* ── Col 1: Category sidebar ── */}
      <div className={FY_SIDEBAR}>
        <div className={FY_SEARCH_WRAP}>
          <FoundrySearch placeholder="Search (comma-separated)…" value={inputSearch}
            onChange={e => setInputSearch(e.target.value)} />
        </div>
        {CRAFT_CATEGORIES.map(cat => (
          <CategoryButton key={cat} active={activeCat === cat} label={cat}
            className={FY_SIDEBAR_CAT} labelClassName={FY_CAT_LABEL}
            onClick={() => onFiltersChange({ ...filters, activeCat: cat, search: "" })}>
            {categoryCounts[cat] ? (
              <span className={CAT_COUNT}><span className={CAT_TOTAL}>{categoryCounts[cat]}</span></span>
            ) : null}
          </CategoryButton>
        ))}
      </div>

      {/* ── Col 2: Card grid ── */}
      <div className={FY_MAIN}>
        <FilterBar>
          <FilterChip active={filterPrime} onClick={() => set("filterPrime", !filterPrime)}>Prime</FilterChip>
          <FilterChip active={filterNonPrime} onClick={() => set("filterNonPrime", !filterNonPrime)}>Non-Prime</FilterChip>
          <FilterChip active={filterVaulted} onClick={() => set("filterVaulted", !filterVaulted)}>🔒 Vaulted</FilterChip>
          <FilterChip active={filterUnvaulted} onClick={() => set("filterUnvaulted", !filterUnvaulted)}>🔓 Unvaulted</FilterChip>
          <FilterSeparator />
          <FilterChip active={filterOwned} onClick={() => set("filterOwned", !filterOwned)}>✓ Owned</FilterChip>
          <FilterChip active={filterUnowned} onClick={() => set("filterUnowned", !filterUnowned)}>✕ Unowned</FilterChip>
          <FilterChip active={ignoreFormaKuva} onClick={() => set("ignoreFormaKuva", !ignoreFormaKuva)} title="Treat Forma and Kuva as always owned when filtering">Ignore Forma/Kuva</FilterChip>
          <FilterChip active={filterReady} onClick={() => set("filterReady", !filterReady)}>⚡ Ready</FilterChip>
          <FilterSeparator />
          <FilterChip active={filterMastered} onClick={() => onFiltersChange({ ...filters, filterMastered: !filterMastered, filterUnmastered: false })}>★ Mastered</FilterChip>
          <FilterChip active={filterUnmastered} onClick={() => onFiltersChange({ ...filters, filterUnmastered: !filterUnmastered, filterMastered: false })}>☆ Unmastered</FilterChip>
          <FilterSeparator />
          <FilterChip active={filterLvlCap} onClick={() => onFiltersChange({ ...filters, filterLvlCap: !filterLvlCap, ...(!filterLvlCap ? { activeCat: "All" } : {}) })}>Lvl &gt; 30</FilterChip>
          <FilterSeparator />
          <FilterPresets module="foundry" {...{ filters, onFiltersChange, filterPresets, onFilterPresetsChange, onOpenSettings }} />
          <span className="ml-auto text-[11px] text-muted">{visible.length} items</span>
          <ViewToggle view={craftView} onChange={v => { setCraftView(v); localStorage.setItem(PREFERENCE_KEYS.FOUNDRY_VIEW, v); }} />
          <HelpTip items={[
            { swatch: "rgba(240,192,64,.5)", icon: "✓✓", label: "Owned",          desc: "Gold border + ✓✓ — item built and in inventory" },
            { swatch: "rgba(56,139,253,.5)", icon: "⚡",  label: "Ready to craft", desc: "Blue border + ⚡ — all parts collected" },
            { swatch: "rgba(240,192,64,.4)", icon: "BP",  label: "Blueprint",      desc: "Gold comp row — blueprint in inventory" },
            { swatch: "rgba(63,185,80,.4)",  icon: "✓",   label: "Part owned",     desc: "Green comp row — component in inventory" },
            { icon: "★",  label: "★ Mastered", desc: "Item levelled to rank 30" },
            { icon: "⚒",  label: "⚒ Building", desc: "Currently crafting in the Foundry" },
            { icon: "MR", label: "MR{n}",       desc: "Required Mastery Rank to use" },
          ]} />
        </FilterBar>

        <div className={craftGridClass(craftView)}>
          {visible.length === 0 && (
            <EmptyMessage>
              {craftable.length === 0 ? "No recipes loaded — refresh item list first." : "No items match."}
            </EmptyMessage>
          )}
          {pagedItems.map(item => (
            <CraftCard
              key={item.unique_name}
              item={item}
              recipe={recipes.has(item.unique_name) ? recipes.get(item.unique_name)! : null}
              inventory={inventory}
              relicDrops={relicDrops}
              relicNames={relicNames}
              crafting={crafting}
              isTracked={trackedSet.has(item.unique_name)}
              onTrack={handleTrack}
              onOpen={handleOpen}
              subsummedWarframes={subsummedWarframes}
              view={craftView}
            />
          ))}
        </div>
        {pageCount > 1 && (
          <div className={FY_PAGINATION}>
            <SecondaryButton disabled={page === 0} onClick={() => setPage(p => p - 1)}>← Prev</SecondaryButton>
            <span className={FY_PG_LABEL}>Page {page + 1} of {pageCount}</span>
            <SecondaryButton disabled={page >= pageCount - 1} onClick={() => setPage(p => p + 1)}>Next →</SecondaryButton>
          </div>
        )}
      </div>

    </div>
  );
}
