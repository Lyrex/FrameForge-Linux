import { fmt } from "../utils";
import type { CraftRow } from "../lib/craftPlan";

export function CraftCounts({ row, className }: { row: CraftRow; className: string }) {
  return (
    <span className={className}>
      <span className={row.from_stock >= row.needed ? "qty-have" : "qty-need"}>{fmt(row.from_stock)}</span>
      <span className="qty-sep">/</span>
      <span className="qty-required">{fmt(row.needed)}</span>
      {row.short > 0 && <span className="recipe-shortage">−{fmt(row.short)}</span>}
      {row.crafts > 0 && <span className="recipe-build" title="Build first">⚒ ×{fmt(row.crafts)}</span>}
    </span>
  );
}
