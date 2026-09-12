import { useState } from "react";
import type { InventoryItem } from "../types/items";
import Syndicates from "./Syndicates";
import Mastery from "./Mastery";
import { SYNDICATE_FILTERS_DEFAULT } from "../constants/filters";
import type { SyndicateFilters } from "../types/filters";

export type CompletionistView = "syndicates" | "mastery";

interface CompletionistTabsProps {
  inventory: Record<string, InventoryItem>;
  refreshKey: number;
}

export default function CompletionistTabs({ inventory, refreshKey }: CompletionistTabsProps) {
  const [view, setView] = useState<CompletionistView>("syndicates");
  const [syndicateFilters, setSyndicateFilters] = useState<SyndicateFilters>(SYNDICATE_FILTERS_DEFAULT);

  return (
    <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>
      <div style={{ display: "flex", gap: 2, padding: "8px 12px 0", borderBottom: "1px solid var(--border)", flexShrink: 0 }}>
        {(["syndicates", "mastery"] as const).map(tab => (
          <button
            key={tab}
            onClick={() => setView(tab)}
            style={{
              padding: "5px 16px", border: "none", borderRadius: "6px 6px 0 0",
              borderBottom: `3px solid ${view === tab ? "var(--accent, #888)" : "transparent"}`,
              background: view === tab ? "var(--bg-card)" : "transparent",
              color: view === tab ? "var(--text)" : "var(--text-dim)", cursor: "pointer",
              fontSize: 13, fontWeight: 500, marginBottom: -1,
              transition: "background 0.15s, color 0.15s", textTransform: "capitalize",
            }}
          >
            {tab === "syndicates" ? "Syndicates" : "Mastery"}
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
      {view === "mastery" && <Mastery inventory={inventory} refreshKey={refreshKey} />}
    </div>
  );
}
