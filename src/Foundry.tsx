import { useState, useEffect, useMemo, useCallback, memo, startTransition, useRef, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import ItemImg from "./ItemImg";
import { useModal } from "./shared/useModal";
import { HelpTip } from "./shared/HelpTip";
import FilterPresets from "./shared/FilterPresets";
import { PREFERENCE_KEYS } from "./constants/preferences";
import { matchesSearchTerms, splitSearchTerms } from "./lib/search";
import { WARFRAME_WIKI_BASE } from "./constants/urls";
import { TAURI_COMMANDS } from "./constants/tauri";
import type { ArchonShard, CatalogItem, CraftingJob, InventoryItem, RecipeComponent, RecipeMap, RelicDropMap } from "./types/items";
import type { CraftPlan } from "./types/mastery";
import { componentStatus, craftableNow, craftRows, distinct, type CraftRow } from "./lib/craftPlan";
import { usePlanCrafts } from "./shared/usePlanCrafts";
import { CraftCounts } from "./shared/CraftCounts";
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

const isReady = (plan: CraftPlan | undefined) => !!plan && craftableNow(plan);

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
        className="craft-icon-tag craft-icon-archon"
        title={`Archon Shards:\n${lines}`}
        style={{ padding: 0 }}
      >
        <svg width="18" height="18" viewBox="0 0 16 16" fill="none" style={{ display: "block" }}>
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
    <svg viewBox="0 0 20 26" width="11" height="14" fill="none" xmlns="http://www.w3.org/2000/svg" className="relic-icon">
      <ellipse cx="10" cy="13" rx="8.5" ry="11.5" fill="rgba(255,220,100,.15)" stroke="rgba(255,220,100,.7)" strokeWidth="1.2"/>
      <path d="M10 4 C7 7 6 10 8 13 C10 16 9 19 10 22" stroke="rgba(255,220,100,.9)" strokeWidth="1.3" strokeLinecap="round" fill="none"/>
      <path d="M10 4 C13 7 14 10 12 13 C10 16 11 19 10 22" stroke="rgba(255,220,100,.6)" strokeWidth="0.9" strokeLinecap="round" fill="none"/>
    </svg>
  );
}

function FormaIcon({ count }: { count: number }) {
  return (
    <span className="craft-icon-tag craft-icon-forma" title={`${count} Forma applied`}>
      <span className="craft-icon-forma-img-wrap">
        <img src={formaIcon} alt="" className="craft-icon-forma-img" />
      </span>
      <span className="craft-icon-forma-count">{count}</span>
    </span>
  );
}

const RELIC_SUFFIXES = ["Bronze", "Silver", "Gold", "Platinum"];
function ownsRelicVariant(relicUnique: string, inventory: Record<string, InventoryItem>): boolean {
  const base = relicUnique.replace(/(Bronze|Silver|Gold|Platinum)$/, "");
  return RELIC_SUFFIXES.some(s => (inventory[`${base}${s}`]?.quantity ?? 0) > 0);
}

// ─── Comp row (used inside modal tree) ───────────────────────────────────────

function CompRow({ comp, plan, inventory, relicDrops, relicNames }: {
  comp: RecipeComponent; plan: CraftPlan | undefined; inventory: Record<string, InventoryItem>;
  relicDrops: RelicDropMap; relicNames: Record<string, string>;
}) {
  const status = plan ? componentStatus(comp, plan) : "none";
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
    <div className={`comp-row comp-row-${status}`}>
      {ownedRelics.length > 0 && (
        <span className="relic-icon-wrap" title={ownedRelics.join("\n")}><RelicIcon /></span>
      )}
      <span className="comp-row-name">{comp.name}</span>
      {status === "part"      && <span className="comp-row-badge">✓</span>}
      {status === "blueprint" && <span className="comp-row-badge">BP</span>}
    </div>
  );
}

// ─── Tree node (modal recipe tree) ───────────────────────────────────────────

/** A node the plan never reached (its parent came out of stock, or waits on a reusable
 *  blueprint) keeps its place in the tree but shows no counts. */
function TreeNode({ node, rows, depth }: {
  node: RecipeComponent; rows: CraftRow[]; depth: number;
}) {
  const row = rows.find(r => r.unique_name === node.unique_name);
  const hasChildren = node.components.length > 0;
  // Only a built intermediate has children the plan counted, so only those open by default.
  const [open, setOpen] = useState(depth < 3 && (row?.crafts ?? 0) > 0);
  return (
    <div style={{ marginLeft: depth * 16 }}>
      <div
        className={`recipe-row ${!row ? "" : row.short > 0 ? "recipe-missing" : "recipe-ok"}`}
        onClick={() => hasChildren && setOpen(o => !o)}
        style={{ cursor: hasChildren ? "pointer" : "default" }}
      >
        {hasChildren
          ? <span className="recipe-chevron">{open ? "▾" : "▸"}</span>
          : <span className="recipe-chevron recipe-chevron-leaf">·</span>}
        <span className="recipe-name">{node.name}</span>
        {row && <CraftCounts row={row} className="recipe-counts" />}
      </div>
      {hasChildren && open && distinct(node.components).map((child, i) => (
        <TreeNode key={i} node={child} rows={rows} depth={depth + 1} />
      ))}
    </div>
  );
}

// ─── Recipe modal ─────────────────────────────────────────────────────────────

function RecipeModal({ item, recipe, inventory, isTracked, onTrack, onClose, building }: {
  item: CatalogItem; recipe: RecipeComponent[] | null;
  inventory: Record<string, InventoryItem>; isTracked: boolean;
  onTrack: () => void; onClose: () => void; building: Set<string>;
}) {
  const [mode, setMode] = useState<"tree" | "needs">("tree");
  const isKuva     = isLichWeapon(item);
  const isAcquired = !!item.source_type;
  const isCrafting = building.has(item.unique_name);

  const targets = useMemo(() => [item.unique_name], [item.unique_name]);
  const plan = usePlanCrafts(targets, inventory)[item.unique_name];
  const rows = useMemo(() => plan ? craftRows(plan) : [], [plan]);
  const needs = useMemo(() => rows.filter(r => r.short > 0 || r.crafts > 0), [rows]);

  const modal = useModal(onClose);
  return (
    <dialog className="craft-modal-overlay" {...modal}>
      <div className="craft-modal" onClick={e => e.stopPropagation()}>

        {/* Header */}
        <div className="craft-modal-header">
          <ItemImg imageName={item.image_name} category={item.category} size={36} />
          <span className="craft-modal-title">{item.name}</span>
          {isCrafting && <span className="craft-modal-foundry-badge" title={`Building — ${item.name}`}>⚒ Building</span>}
          <button className={`foundry-track-btn-large ${isTracked ? "tracked" : ""}`} onClick={onTrack}>
            {isTracked ? "★ Tracked" : "☆ Track"}
          </button>
          <button className="craft-detail-close" onClick={onClose}>✕</button>
        </div>

        {isKuva ? (
          <div className="craft-modal-body">
            <div className="craft-kuva-notice">
              <span className="craft-kuva-icon">🔱</span>
              <div>
                <strong>{item.name}</strong> is obtained by converting a{" "}
                {item.name.startsWith("Kuva ") ? <strong>Kuva Lich</strong> : <strong>Tenet Sister</strong>},
                not crafted from a Blueprint.
              </div>
            </div>
          </div>
        ) : isAcquired ? (
          <div className="craft-modal-body">
            <div className="craft-kuva-notice">
              <span className="craft-kuva-icon">🎮</span>
              <div>
                <strong>{item.name}</strong> is acquired in-game and cannot be crafted in the Foundry.
              </div>
            </div>
          </div>
        ) : (
          <>
            <div className="craft-modal-tabs">
              <button className={`toggle-btn ${mode === "tree" ? "toggle-active" : ""}`} onClick={() => setMode("tree")}>Full tree</button>
              <button className={`toggle-btn ${mode === "needs" ? "toggle-active" : ""}`} onClick={() => setMode("needs")}>What I need</button>
            </div>
            <div className="craft-modal-body">
              {!recipe || !plan ? (
                <div className="empty-msg">Loading…</div>
              ) : recipe.length === 0 ? (
                <div className="empty-msg">No recipe data.</div>
              ) : mode === "tree" ? (
                distinct(recipe).map((node, i) => <TreeNode key={i} node={node} rows={rows} depth={0} />)
              ) : needs.length === 0 ? (
                <div className="empty-msg">✓ You have everything needed.</div>
              ) : (
                <div className="needs-list">
                  {needs.map(r => (
                    <div key={r.unique_name} className="needs-row">
                      <span className="needs-name">{r.name}</span>
                      <CraftCounts row={r} className="needs-counts" />
                    </div>
                  ))}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </dialog>
  );
}

// ─── Craft card ───────────────────────────────────────────────────────────────

const CraftCard = memo(function CraftCard({ item, recipe, plan, inventory, relicDrops, relicNames, building, isTracked, onTrack, onOpen, subsummedWarframes, view }: {
  item: CatalogItem; recipe: RecipeComponent[] | null; plan: CraftPlan | undefined;
  inventory: Record<string, InventoryItem>; relicDrops: RelicDropMap;
  relicNames: Record<string, string>;
  building: Set<string>; isTracked: boolean;
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
  const isCrafting = building.has(item.unique_name);
  const isKuva     = isLichWeapon(item);
  const parts = recipe ? distinct(recipe) : null;
  const ready = isReady(plan);

  if (view === "icons") {
    return (
      <div className={`craft-icon-card${isOwned ? " craft-card-owned" : ""}${ready && !isOwned ? " craft-card-ready" : ""}`}
        title={`${item.name}${isOwned ? " (owned)" : ready ? " (ready)" : ""}`}
        onClick={() => onOpen(item)}>
        <ItemImg imageName={item.image_name} category={item.category} size={72} />
        {isOwned && <span className="craft-icon-badge craft-icon-badge-owned">✓✓</span>}
        {!isOwned && ready && <span className="craft-icon-badge craft-icon-badge-ready">⚡</span>}
      </div>
    );
  }

  if (view === "list" || view === "list-compact") {
    return (
      <div className={`craft-row${isOwned ? " craft-row-owned" : ""}${ready && !isOwned ? " craft-row-ready" : ""}`}
        onClick={() => onOpen(item)}>
        {view === "list" && (
          <div className="craft-row-icon">
            <ItemImg imageName={item.image_name} category={item.category} size={24} />
          </div>
        )}
        <div className="craft-row-name">{item.name}</div>
        {item.mastery_req != null && item.mastery_req > 0 &&
          <span className="craft-mr-req craft-row-mr">MR {item.mastery_req}</span>}
        <div className="craft-row-status">
          {isMastered && <span className="craft-icon-tag craft-icon-mastered" title="Mastered">★</span>}
          {isOwned && !isMastered && <span className="craft-icon-tag craft-icon-owned">✓✓</span>}
          {!isOwned && ready && <span className="craft-icon-tag craft-icon-ready">⚡</span>}
          {isCrafting && <span className="craft-icon-tag craft-icon-foundry" title="Building">⚒</span>}
          {formaCount > 0 && <FormaIcon count={formaCount} />}
        </div>
        {item.source_type
          ? <span className="craft-row-parts craft-row-acquired-tag">Acquired in-game</span>
          : parts && parts.length > 0
            ? <span className="craft-row-parts">{parts.length} part{parts.length !== 1 ? "s" : ""}</span>
            : null}
      </div>
    );
  }

  if (view === "text-cards") {
    return (
      <div className={`craft-text-card${isOwned ? " craft-card-owned" : ""}${ready && !isOwned ? " craft-card-ready" : ""}`}
        onClick={() => onOpen(item)}>
        <div className="ctc-name">{item.name}</div>
        <div className="ctc-meta">
          {item.vaulted === true  && <span className="vault-badge vault-yes">🔒 Vaulted</span>}
          {item.vaulted === false && <span className="vault-badge vault-no">🔓 Unvaulted</span>}
          {item.mastery_req != null && item.mastery_req > 0 &&
            <span className="craft-mr-req">MR {item.mastery_req}</span>}
        </div>
        <div className="ctc-tags">
          {isMastered && <span className="craft-icon-tag craft-icon-mastered" title="Mastered">★</span>}
          {isOwned && !isMastered && <span className="craft-icon-tag craft-icon-owned">✓✓</span>}
          {!isOwned && ready && <span className="craft-icon-tag craft-icon-ready">⚡</span>}
          {isCrafting && <span className="craft-icon-tag craft-icon-foundry" title="Building">⚒</span>}
          {formaCount > 0 && <FormaIcon count={formaCount} />}
        </div>
      </div>
    );
  }

  return (
    <div
      className={`craft-card${isOwned ? " craft-card-owned" : ""}${ready && !isOwned ? " craft-card-ready" : ""}`}
      onClick={() => onOpen(item)}
    >
      {/* Col 1, rows 1-4: image block with star/wiki/name overlaid */}
      <div className="cc-image">
        <ItemImg imageName={item.image_name} category={item.category} size={78} />
        <button className={`cc-star ${isTracked ? "tracked" : ""}`}
          onClick={e => { e.stopPropagation(); onTrack(item); }}>{isTracked ? "★" : "☆"}</button>
        <button className="cc-wiki"
          onClick={e => { e.stopPropagation(); invoke(TAURI_COMMANDS.OPEN_URL, { url:`${WARFRAME_WIKI_BASE}/${item.name.replace(" Blueprint","").replace(/\s+/g,"_")}` }).catch(()=>{}); }}>wiki</button>
        <span className="cc-name">{item.name}</span>
      </div>

      {/* Col 1, row 5: MR requirement + subsumed indicator */}
      <div className="cc-mr">
        {isSubsumed && <img src={sentientIcon} className="cc-subsumed-icon" title="Subsumed into Helminth" alt="Subsumed" />}
        {item.mastery_req != null && item.mastery_req > 0 &&
          <span className="craft-mr-req">MR {item.mastery_req}</span>}
      </div>

      {/* Col 1, row 6: vault / kuva / acquired badges */}
      <div className="cc-badges">
        {item.vaulted === true  && <span className="vault-badge vault-yes">🔒 Vaulted</span>}
        {item.vaulted === false && <span className="vault-badge vault-no">🔓 Unvaulted</span>}
        {isKuva && <span className="craft-icon-tag craft-icon-kuva" title="Lich/Sister">🔱</span>}
      </div>

      {/* Col 1, row 7: status tags */}
      <div className="cc-tags">
        {isMastered && <span className="craft-icon-tag craft-icon-mastered" title="Mastered">★</span>}
        {isOwned && !isMastered && rank != null && <span className="craft-icon-tag craft-icon-rank">R{rank}</span>}
        {isCrafting  && <span className="craft-icon-tag craft-icon-foundry" title="Building">⚒</span>}
        {formaCount > 0 && <FormaIcon count={formaCount} />}
        {shards.length > 0 && <ArchonCrystalIcon shards={shards} />}
        {isOwned     && <span className="foundry-cb-badge foundry-cb-owned">✓✓</span>}
        {!isOwned && ready && <span className="foundry-cb-badge foundry-cb-ready">⚡</span>}
      </div>

      {/* Col 2, rows 1-7: ingredient list — rows grow to fill available height */}
      <div className="cc-ingredients">
        {recipe === null ? (
          <div className="comp-row-loading">Loading…</div>
        ) : item.source_type ? (
          <div className="comp-row-acquired">Acquired in-game</div>
        ) : recipe.length === 0 ? (
          <div className="comp-row-loading">No recipe</div>
        ) : (
          parts!.map((comp, i) => (
            <CompRow key={i} comp={comp} plan={plan} inventory={inventory} relicDrops={relicDrops} relicNames={relicNames} />
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
  if (prev.building      !== next.building)      return false;
  if (prev.onTrack       !== next.onTrack)       return false;
  if (prev.onOpen        !== next.onOpen)        return false;
  if (prev.relicDrops    !== next.relicDrops)    return false;
  if (prev.relicNames    !== next.relicNames)    return false;
  if (prev.subsummedWarframes !== next.subsummedWarframes) return false;
  // Every re-plan yields fresh plan objects, so compare by content.
  if (JSON.stringify(prev.plan) !== JSON.stringify(next.plan)) return false;
  const k = prev.item.unique_name;
  if ((prev.inventory[k]?.quantity    ?? 0)    !== (next.inventory[k]?.quantity    ?? 0))    return false;
  if ((prev.inventory[k]?.mastery_rank ?? null) !== (next.inventory[k]?.mastery_rank ?? null)) return false;
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
  const [craftable, setCraftable] = useState<CatalogItem[]>([]);
  const [recipes, setRecipes]     = useState<Map<string, RecipeComponent[]>>(new Map());
  const [blueprintResults, setBlueprintResults] = useState<Record<string, string>>({});
  const [relicDrops, setRelicDrops] = useState<RelicDropMap>({});
  const [relicNames, setRelicNames] = useState<Record<string, string>>({});
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
    invoke<CatalogItem[]>(TAURI_COMMANDS.GET_CRAFTABLE_ITEMS).then(setCraftable).catch(() => setCraftable([]));
    invoke<RelicDropMap>("get_relic_drops").then(setRelicDrops).catch(() => {});
    invoke<Record<string, string>>(TAURI_COMMANDS.GET_BLUEPRINT_RESULTS).then(setBlueprintResults).catch(() => {});
    invoke<CatalogItem[]>(TAURI_COMMANDS.GET_ALL_ITEMS)
      .then(items => {
        const map: Record<string, string> = {};
        for (const i of items) if (i.category === "Relics") map[i.unique_name] = i.name;
        setRelicNames(map);
      }).catch(() => {});
  }, [refreshKey]);

  // A Foundry job carries the blueprint path, so it is resolved to the item it builds before matching catalog items.
  const building = useMemo(() =>
    new Set(crafting.map(c => blueprintResults[c.unique_name] ?? c.unique_name)),
    [crafting, blueprintResults]);

  const candidates = useMemo(() => {
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
      .filter(i => !filterReady || (inventory[i.unique_name]?.quantity ?? 0) === 0)
      .filter(i => !filterLvlCap || (i.max_level_cap != null && i.max_level_cap > 30));
  }, [craftable, activeCat, search, filterPrime, filterNonPrime, filterVaulted, filterUnvaulted,
      filterMastered, filterUnmastered, filterOwned, filterUnowned, filterReady, filterLvlCap, ignoreFormaKuva,
      // Only pull in inventory when a filter that actually reads it is active.
      // Without this guard, every 10-second scanner update re-renders all 100+ cards.
      (filterMastered || filterUnmastered || filterOwned || filterUnowned || filterReady) ? inventory : null,
  ]);

  const PAGE_SIZE = pageSize;
  const pageOf = useCallback((items: CatalogItem[]) => items.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE), [page, PAGE_SIZE]);

  // One standalone plan per card, so a badge agrees with the modal, which plans its item alone.
  // When the ⚡ filter is on every candidate needs a plan, and otherwise only the page does.
  const planTargets = useMemo(() => (filterReady ? candidates : pageOf(candidates)).map(i => i.unique_name), [candidates, filterReady, pageOf]);
  const plans = usePlanCrafts(planTargets, inventory, true);

  const visible = useMemo(() => filterReady ? candidates.filter(i => isReady(plans[i.unique_name])) : candidates, [candidates, filterReady, plans]);

  // Reset to page 1 when the user changes a filter, search, or category —
  // but NOT when inventory or recipes update in the background.
  // Without this guard, every 10-second scan resets the page mid-browse.
  useEffect(() => { setPage(0); }, [ // eslint-disable-line
    activeCat, search, filterPrime, filterNonPrime, filterVaulted, filterUnvaulted,
    filterMastered, filterUnmastered, filterOwned, filterUnowned, filterReady, filterLvlCap, ignoreFormaKuva,
    craftable, pageSize,
  ]);
  const pageCount = Math.ceil(visible.length / PAGE_SIZE);
  const pagedItems = useMemo(() => pageOf(visible), [visible, pageOf]);

  // Load recipes for visible items — one bulk IPC call instead of N concurrent calls
  useEffect(() => {
    const toLoad = visible.filter(i => !recipes.has(i.unique_name));
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
  }, [visible]);

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

  return (
    <div className="foundry">

      {/* ── Modal overlay ── */}
      {modalItem && (
        <RecipeModal
          item={modalItem}
          recipe={modalRecipe}
          inventory={inventory}
          isTracked={tracked.includes(modalItem.unique_name)}
          onTrack={() => handleTrack(modalItem)}
          onClose={() => setModalItem(null)}
          building={building}
        />
      )}

      {/* ── Col 1: Category sidebar ── */}
      <div className="foundry-sidebar">
        <div className="foundry-search-wrap">
          <input className="foundry-search" placeholder="Search (comma-separated)…" value={inputSearch}
            onChange={e => setInputSearch(e.target.value)} />
        </div>
        {CRAFT_CATEGORIES.map(cat => (
          <button key={cat} className={`cat-btn ${activeCat === cat ? "cat-active" : ""}`}
            onClick={() => onFiltersChange({ ...filters, activeCat: cat, search: "" })}>
            <span className="cat-label">{cat}</span>
            {categoryCounts[cat] ? (
              <span className="cat-count"><span className="cat-total">{categoryCounts[cat]}</span></span>
            ) : null}
          </button>
        ))}
      </div>

      {/* ── Col 2: Card grid ── */}
      <div className="foundry-main">
        <div className="filter-bar">
          <button className={`fchip ${filterPrime    ? "fchip-on" : ""}`} onClick={() => set("filterPrime", !filterPrime)}>Prime</button>
          <button className={`fchip ${filterNonPrime ? "fchip-on" : ""}`} onClick={() => set("filterNonPrime", !filterNonPrime)}>Non-Prime</button>
          <button className={`fchip ${filterVaulted   ? "fchip-on" : ""}`} onClick={() => set("filterVaulted", !filterVaulted)}>🔒 Vaulted</button>
          <button className={`fchip ${filterUnvaulted ? "fchip-on" : ""}`} onClick={() => set("filterUnvaulted", !filterUnvaulted)}>🔓 Unvaulted</button>
          <span className="fbar-sep"/>
          <button className={`fchip ${filterOwned     ? "fchip-on" : ""}`} onClick={() => set("filterOwned", !filterOwned)}>✓ Owned</button>
          <button className={`fchip ${filterUnowned   ? "fchip-on" : ""}`} onClick={() => set("filterUnowned", !filterUnowned)}>✕ Unowned</button>
          <button className={`fchip ${ignoreFormaKuva ? "fchip-on" : ""}`} onClick={() => set("ignoreFormaKuva", !ignoreFormaKuva)} title="Treat Forma and Kuva as always owned when filtering">Ignore Forma/Kuva</button>
          <button className={`fchip ${filterReady     ? "fchip-on" : ""}`} onClick={() => set("filterReady", !filterReady)}>⚡ Ready</button>
          <span className="fbar-sep"/>
          <button className={`fchip ${filterMastered  ? "fchip-on" : ""}`} onClick={() => onFiltersChange({ ...filters, filterMastered: !filterMastered, filterUnmastered: false })}>★ Mastered</button>
          <button className={`fchip ${filterUnmastered? "fchip-on" : ""}`} onClick={() => onFiltersChange({ ...filters, filterUnmastered: !filterUnmastered, filterMastered: false })}>☆ Unmastered</button>
          <span className="fbar-sep"/>
          <button className={`fchip ${filterLvlCap   ? "fchip-on" : ""}`} onClick={() => onFiltersChange({ ...filters, filterLvlCap: !filterLvlCap, ...(!filterLvlCap ? { activeCat: "All" } : {}) })}>Lvl &gt; 30</button>
          <span className="fbar-sep"/>
          <FilterPresets module="foundry" {...{ filters, onFiltersChange, filterPresets, onFilterPresetsChange, onOpenSettings }} />
          <span style={{ marginLeft: "auto", fontSize: 11, color: "var(--muted)" }}>{visible.length} items</span>
          <ViewToggle view={craftView} onChange={v => { setCraftView(v); localStorage.setItem(PREFERENCE_KEYS.FOUNDRY_VIEW, v); }} />
          <HelpTip items={[
            { swatch: "rgba(240,192,64,.5)", icon: "✓✓", label: "Owned",          desc: "Gold border + ✓✓ — item built and in inventory" },
            { swatch: "rgba(56,139,253,.5)", icon: "⚡",  label: "Ready to craft", desc: "Blue border + ⚡ — all parts collected" },
            { swatch: "rgba(240,192,64,.4)", icon: "BP",  label: "Blueprint",      desc: "Gold comp row — blueprint in inventory" },
            { swatch: "rgba(63,185,80,.4)",  icon: "✓",   label: "Part owned",     desc: "Green comp row — component in inventory" },
            { icon: "★",  label: "★ Mastered", desc: "Item levelled to its max rank" },
            { icon: "⚒",  label: "⚒ Building", desc: "Currently crafting in the Foundry" },
            { icon: "MR", label: "MR{n}",       desc: "Required Mastery Rank to use" },
          ]} />
        </div>

        <div className={`craft-grid craft-grid-${craftView}`}>
          {visible.length === 0 && (
            <div className="empty-msg">
              {craftable.length === 0 ? "No recipes loaded — refresh item list first." : "No items match."}
            </div>
          )}
          {pagedItems.map(item => (
            <CraftCard
              key={item.unique_name}
              item={item}
              recipe={recipes.has(item.unique_name) ? recipes.get(item.unique_name)! : null}
              plan={plans[item.unique_name]}
              inventory={inventory}
              relicDrops={relicDrops}
              relicNames={relicNames}
              building={building}
              isTracked={trackedSet.has(item.unique_name)}
              onTrack={handleTrack}
              onOpen={handleOpen}
              subsummedWarframes={subsummedWarframes}
              view={craftView}
            />
          ))}
        </div>
        {pageCount > 1 && (
          <div className="foundry-pagination">
            <button className="btn-secondary" disabled={page === 0} onClick={() => setPage(p => p - 1)}>← Prev</button>
            <span className="foundry-pg-label">Page {page + 1} of {pageCount}</span>
            <button className="btn-secondary" disabled={page >= pageCount - 1} onClick={() => setPage(p => p + 1)}>Next →</button>
          </div>
        )}
      </div>

    </div>
  );
}
