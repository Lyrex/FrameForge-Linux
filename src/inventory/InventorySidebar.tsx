import { CategoryButton, CAT_COUNT, CAT_OWNED, CAT_SEP, CAT_TOTAL } from "../shared/ui/CategoryButton";

const SIDEBAR = "w-[var(--sidebar-w)] shrink-0 bg-surface border-r border-border flex flex-col overflow-y-auto py-[12px]";
const SECTION_LABEL = "px-[12px] pb-[6px] text-[10px] font-semibold uppercase tracking-[.06em] text-muted";
const SIDEBAR_DIVIDER = "border-t border-border my-[10px]";
const DB_COUNT = "px-[12px] pb-[6px] text-muted text-[12px]";
const BTN_FETCH =
  "mx-[12px] py-[6px] bg-background border border-border rounded-[6px] text-muted cursor-pointer text-[12px] transition-[border-color,color] duration-150 hover:enabled:border-accent hover:enabled:text-accent disabled:opacity-50 disabled:cursor-default";
const FETCH_MSG = "pt-[6px] px-[12px] pb-0 text-[11px] text-muted break-words";

interface InventorySidebarProps {
  categories: { id: string; label: string }[];
  category: string;
  categoryCounts: { owned: Record<string, number>; total: Record<string, number> };
  onCategoryChange: (category: string) => void;
  itemCount: number;
  recipeCount: number;
  onFetch: () => void | Promise<void>;
  fetching: boolean;
  fetchMsg: string;
}

export default function InventorySidebar({
  categories, category, categoryCounts, onCategoryChange, itemCount, recipeCount, onFetch, fetching, fetchMsg,
}: InventorySidebarProps) {
  return (
    <aside className={SIDEBAR}>
      <div className={SECTION_LABEL}>Categories</div>
      {categories.map(cat => {
        const owned = categoryCounts.owned[cat.id] ?? 0;
        const total = categoryCounts.total[cat.id] ?? 0;
        return (
          <CategoryButton key={cat.id} active={category === cat.id} label={cat.label} onClick={() => onCategoryChange(cat.id)}>
            <span className={CAT_COUNT}>
              {owned > 0 ? <span className={CAT_OWNED}>{owned}</span> : null}
              {owned > 0 && <span className={CAT_SEP}>/</span>}
              <span className={CAT_TOTAL}>{total}</span>
            </span>
          </CategoryButton>
        );
      })}
      <div className={SIDEBAR_DIVIDER} />
      <div className={SECTION_LABEL}>Item Database</div>
      <div className={DB_COUNT}>{itemCount.toLocaleString()} items · {recipeCount.toLocaleString()} recipes</div>
      <button className={BTN_FETCH} onClick={onFetch} disabled={fetching}>
        {fetching ? "Fetching…" : "Refresh item list"}
      </button>
      {fetchMsg && <div className={FETCH_MSG}>{fetchMsg}</div>}
    </aside>
  );
}
