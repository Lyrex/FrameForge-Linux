import { memo } from "react";
import ItemImg from "./ItemImg";
import type { ViewMode } from "./ViewToggle";
import { fmt, deltaClass, deltaText } from "./utils";
import "./InventoryGrid.css";

// ─── Types ──────────────────────────────────────────────────────────────────

export type { ViewMode } from "./ViewToggle";

export interface InventoryGridItem {
  unique_name: string;
  name: string;
  category: string;
  image_name?: string | null;
  qty: number;
}

interface InventoryGridProps {
  items: InventoryGridItem[];
  loading: boolean;
  monitoring: boolean;
  view: ViewMode;
  inventory: Record<string, { mastery_rank: number }>;
  modCopies: Record<string, { rank: number | null; count: number }[]>;
  favorites: Set<string>;
  lastChanged: Record<string, number>;
  changes: Map<string, { delta: number }>;
  crafting: Map<string, { item_name: string }>;
  filterRank: string | number | null;
  onToggleFavorite: (id: string) => void;
  onContextMenu?: (e: React.MouseEvent) => void;
}

// ─── Memoized inventory card components ──────────────────────────────────────

interface InvModCardProps {
  unique_name: string;
  name: string;
  category: string;
  image_name?: string | null;
  ranks: { rank: number; count: number }[];
  total: number;
  view: ViewMode;
}
const InvModCard = memo(function InvModCard({ unique_name, name, category, image_name, ranks, total, view }: InvModCardProps) {
  if (view === "icons") {
    return (
      <div key={unique_name} className="inv-card inv-card-icon-only" title={`${name} ×${fmt(total)}`}>
        <ItemImg imageName={image_name ?? undefined} category={category} size={52} />
      </div>
    );
  }
  if (view === "list" || view === "list-compact") {
    return (
      <div key={unique_name} className="inv-card inv-card-row">
        {view === "list" && <div className="inv-row-icon"><ItemImg imageName={image_name ?? undefined} category={category} size={20} /></div>}
        <div className="inv-row-name">{name}</div>
        <div className="inv-row-cat">{category}</div>
        <div className="inv-row-qty">{fmt(total)}</div>
      </div>
    );
  }
  return (
    <div key={unique_name} className="inv-card inv-card-mod">
      {view !== "text-cards" && (
        <div className="inv-card-img-wrap">
          <ItemImg imageName={image_name ?? undefined} category={category} size={40} />
        </div>
      )}
      <div className="inv-card-name">{name}</div>
      <div className="inv-card-cat">{category}</div>
      <div className="mod-rank-table">
        {ranks.map(r => (
          <div key={r.rank} className={`mod-rank-row${r.count === 0 ? " mod-rank-zero" : ""}`}>
            <span className="mod-rank-label">R{r.rank}</span>
            <span className="mod-rank-count">{r.count}</span>
          </div>
        ))}
      </div>
      <div className="inv-card-qty mod-total">{fmt(total)}</div>
    </div>
  );
}, (prev, next) =>
  prev.view === next.view &&
  prev.unique_name === next.unique_name &&
  prev.name === next.name &&
  prev.total === next.total &&
  prev.image_name === next.image_name &&
  prev.ranks.length === next.ranks.length &&
  prev.ranks.every((r, i) => r.rank === next.ranks[i].rank && r.count === next.ranks[i].count)
);

interface InvCardProps {
  unique_name: string;
  name: string;
  category: string;
  image_name?: string | null;
  qty: number;
  isFavorite: boolean;
  changedAt: number | undefined;
  recentDelta: number | null;
  craftJobName: string | null;
  masteryRank: number | undefined;
  onToggleFavorite: (id: string) => void;
  view: ViewMode;
}
const InvCard = memo(function InvCard({
  unique_name, name, category, image_name, qty,
  isFavorite, changedAt, recentDelta, craftJobName, masteryRank, onToggleFavorite, view,
}: InvCardProps) {
  const nowSec = Date.now() / 1000;
  const secAgo = changedAt != null ? nowSec - changedAt : null;
  const isRecent = secAgo !== null && secAgo < 300;
  const isZero = qty === 0 && !craftJobName;
  const isMastered = masteryRank != null && masteryRank >= 30;
  const showRank = masteryRank != null && masteryRank > 0;
  const recentLabel = secAgo !== null ? (Math.floor(secAgo / 60) === 0 ? "· now" : `· ${Math.floor(secAgo / 60)}m`) : null;
  const baseClass = `inv-card${isZero ? " inv-card-zero" : ""}${isRecent ? (recentDelta != null && recentDelta > 0 ? " inv-card-gained" : " inv-card-lost") : ""}`;

  if (view === "icons") {
    return (
      <div className={`${baseClass} inv-card-icon-only`} title={`${name} (${fmt(qty)})`}>
        <ItemImg imageName={image_name ?? undefined} category={category} size={52} />
      </div>
    );
  }
  if (view === "list" || view === "list-compact") {
    return (
      <div className={`${baseClass} inv-card-row`}>
        <button className={`inv-fav-star-row ${isFavorite ? "active" : ""}`}
          title={isFavorite ? "Remove from Modular Window" : "Add to Modular Window"}
          onClick={e => { e.stopPropagation(); onToggleFavorite(unique_name); }}>
          {isFavorite ? "★" : "☆"}
        </button>
        {view === "list" && (
          <div className="inv-row-icon">
            <ItemImg imageName={image_name ?? undefined} category={category} size={20} />
            {craftJobName && <span className="inv-foundry-icon-row" title={`Building — ${craftJobName}`}>⚒</span>}
          </div>
        )}
        <div className="inv-row-name">
          {name}
          {isRecent && <span className="item-updated">{recentLabel}</span>}
        </div>
        <div className="inv-row-cat">{category}</div>
        <div className="inv-row-qty">
          {fmt(qty)}
          {isRecent && recentDelta != null && <span className={`item-delta ${deltaClass(recentDelta)}`}>{deltaText(recentDelta)}</span>}
        </div>
      </div>
    );
  }
  return (
    <div className={baseClass}>
      <button
        className={`inv-fav-star ${isFavorite ? "active" : ""}`}
        title={isFavorite ? "Remove from Modular Window" : "Add to Modular Window"}
        onClick={e => { e.stopPropagation(); onToggleFavorite(unique_name); }}
      >{isFavorite ? "★" : "☆"}</button>
      <div className="inv-mastery-row">
        {isMastered
          ? <span className="inv-mastery-star" title="Mastered">★</span>
          : showRank
            ? <span className="inv-mastery-rank" title={`Rank ${masteryRank}`}>R{masteryRank}</span>
            : null}
      </div>
      {view !== "text-cards" && (
        <div className="inv-card-img-wrap">
          <ItemImg imageName={image_name ?? undefined} category={category} size={48} />
          {craftJobName && <span className="inv-foundry-icon" title={`Building — ${craftJobName}`}>⚒</span>}
        </div>
      )}
      <div className="inv-card-name">
        {name}
        {isRecent && <span className="item-updated">{recentLabel}</span>}
      </div>
      <div className="inv-card-cat">{category}</div>
      <div className={`inv-card-qty ${isZero ? "inv-card-qty-zero" : ""}`}>
        {fmt(qty)}
        {isRecent && recentDelta != null && (
          <span className={`item-delta ${deltaClass(recentDelta)}`}>{deltaText(recentDelta)}</span>
        )}
      </div>
    </div>
  );
}, (prev, next) => {
  if (prev.view !== next.view) return false;
  if (
    prev.unique_name !== next.unique_name ||
    prev.qty !== next.qty ||
    prev.isFavorite !== next.isFavorite ||
    prev.image_name !== next.image_name ||
    prev.masteryRank !== next.masteryRank ||
    prev.craftJobName !== next.craftJobName ||
    prev.recentDelta !== next.recentDelta ||
    prev.changedAt !== next.changedAt
  ) return false;
  // Recently-changed items must re-render so elapsed time stays fresh
  const nowSec = Date.now() / 1000;
  if (prev.changedAt != null && nowSec - prev.changedAt < 300) return false;
  return true;
});

// ─── Main grid component ────────────────────────────────────────────────────

export default function InventoryGrid({
  items, loading, monitoring, view,
  inventory, modCopies, favorites, lastChanged, changes, crafting,
  filterRank, onToggleFavorite, onContextMenu,
}: InventoryGridProps) {
  return (
    <div className={`item-grid item-grid-${view}`}
         onContextMenu={onContextMenu}>
      {loading ? (
        Array.from({ length: 20 }, (_, i) => (
          <div key={i} className="inv-card inventory-skeleton" />
        ))
      ) : items.length === 0 ? (
        <div className="empty-msg" style={{gridColumn:"1/-1"}}>
          {monitoring
            ? "No items found. Complete a mission or visit a relay to sync inventory."
            : "Start the monitor to begin tracking your inventory."}
        </div>
      ) : (
        items.flatMap(item => {
          // Mods & Arcanes: single card with inline rank breakdown
          if ((item.category === "Mods" || item.category === "Arcanes") && modCopies[item.unique_name]) {
            const copies = modCopies[item.unique_name];
            const byRank: Record<number, number> = {};
            for (const c of copies) byRank[c.rank ?? 0] = (byRank[c.rank ?? 0] ?? 0) + c.count;
            const maxRank = Math.max(...Object.keys(byRank).map(Number));
            const ranks = Array.from({ length: maxRank + 1 }, (_, r) => ({ rank: r, count: byRank[r] ?? 0 })).filter(r => r.count > 0);
            if (filterRank !== null) {
              const targetRank = filterRank === "unranked" ? 0 : filterRank as number;
              if ((byRank[targetRank] ?? 0) === 0) return [];
            }
            const total = Object.values(byRank).reduce((a, b) => a + b, 0);
            return [(
              <InvModCard key={item.unique_name}
                unique_name={item.unique_name} name={item.name}
                category={item.category} image_name={item.image_name}
                ranks={ranks} total={total} view={view} />
            )];
          }

          // Normal item card
          const changedAt = lastChanged[item.unique_name];
          const recentChange = changedAt != null ? changes.get(item.unique_name) : undefined;
          const craftJob = crafting.get(item.unique_name);
          return [(
            <InvCard key={item.unique_name}
              unique_name={item.unique_name} name={item.name}
              category={item.category} image_name={item.image_name}
              qty={item.qty}
              isFavorite={favorites.has(item.unique_name)}
              changedAt={changedAt}
              recentDelta={recentChange?.delta ?? null}
              craftJobName={craftJob?.item_name ?? null}
              masteryRank={inventory[item.unique_name]?.mastery_rank}
              onToggleFavorite={onToggleFavorite}
              view={view} />
          )];
        })
      )}
    </div>
  );
}
