import { useState } from "react";
import type { InventoryItem } from "../types/items";
import Syndicates from "./Syndicates";
import Weapons from "./Weapons";
import { SYNDICATE_FILTERS_DEFAULT } from "../constants/filters";
import type { SyndicateFilters } from "../types/filters";

export type CompletionistView = "syndicates" | "weapons";
type WeaponTab = "Primary" | "Secondary" | "Melee" | "Operator";

interface CompletionistTabsProps {
  inventory: Record<string, InventoryItem>;
}

export default function CompletionistTabs({ inventory }: CompletionistTabsProps) {
  const [view, setView] = useState<CompletionistView>("syndicates");
  const [weaponsTab, setWeaponsTab] = useState<WeaponTab>("Primary");
  const [syndicateFilters, setSyndicateFilters] = useState<SyndicateFilters>(SYNDICATE_FILTERS_DEFAULT);

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex shrink-0 gap-[2px] border-b border-border px-3 pt-2">
        {(["syndicates", "weapons"] as const).map(tab => (
          <button
            key={tab}
            onClick={() => setView(tab)}
            className={`-mb-px cursor-pointer rounded-t-[6px] border-0 border-b-[3px] px-4 py-[5px] text-[13px] font-medium capitalize transition-[background,color] duration-150 ${view === tab ? "border-accent bg-[var(--bg-card)] text-foreground" : "border-transparent bg-transparent text-[var(--text-dim)]"}`}
          >
            {tab === "syndicates" ? "Syndicates" : "Weapons"}
          </button>
        ))}
      </div>
      {view === "syndicates" && (
        <Syndicates
          inventory={inventory}
          filters={syndicateFilters}
          onFiltersChange={setSyndicateFilters}
        />
      )}
      {view === "weapons" && (
        <Weapons
          inventory={inventory}
          activeTab={weaponsTab}
          onTabChange={setWeaponsTab}
        />
      )}
    </div>
  );
}
