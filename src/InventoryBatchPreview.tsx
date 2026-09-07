import { useState } from "react";
import InventoryGrid from "./InventoryGrid";
import { ViewToggle, type ViewMode } from "./ViewToggle";
import "./InventoryBatchPreview.css";

interface InventoryBatchPreviewProps {
  onClose: () => void;
}

const PREVIEW_STATES = [
  ["all", "All incoming"],
  ["gained", "Gained"],
  ["lost", "Lost"],
  ["crafting", "Crafting"],
  ["mod-transfer", "Rank transfer"],
  ["mod-gained", "Rank gain"],
  ["expired", "After 5 min"],
] as const;

type PreviewState = typeof PREVIEW_STATES[number][0];

export default function InventoryBatchPreview({ onClose }: InventoryBatchPreviewProps) {
  const [view, setView] = useState<ViewMode>("cards");
  const [favorites, setFavorites] = useState<Set<string>>(new Set());
  const [previewState, setPreviewState] = useState<PreviewState>("all");
  const now = Math.floor(Date.now() / 1000);
  const expired = previewState === "expired";
  const items = [
    { unique_name: "preview-gained", name: "New resource stack", category: "Resources", qty: 12, state: "gained" },
    { unique_name: "preview-lost", name: "Consumed component", category: "Misc", qty: 0, state: "lost" },
    { unique_name: "preview-crafting", name: "Building item", category: "Primary", qty: 1, state: "crafting" },
    { unique_name: "preview-mod-transfer", name: "Rank transfer", category: "Mods", qty: 2, state: "mod-transfer" },
    { unique_name: "preview-mod-gained", name: "Ranked mod gain", category: "Mods", qty: 3, state: "mod-gained" },
  ];

  const changes = new Map([
    ["preview-gained", [{ delta: 12 }]],
    ["preview-lost", [{ delta: -3 }]],
    ["preview-mod-transfer", [{ rank: 0, delta: -1 }, { rank: 5, delta: 1 }]],
    ["preview-mod-gained", [{ rank: 3, delta: 3 }]],
  ]);

  return (
    <div className="inventory-preview-overlay" onClick={onClose}>
      <section className="inventory-preview" onClick={event => event.stopPropagation()} aria-label="Inventory change preview">
        <header className="inventory-preview-header">
          <div>
            <strong>Incoming inventory batch preview</strong>
            <span>{expired ? "Recent indicators have expired." : "This does not change inventory data."}</span>
          </div>
          <ViewToggle view={view} onChange={setView} />
          <button className="craft-detail-close" onClick={onClose} aria-label="Close preview">x</button>
        </header>
        <div className="inventory-preview-states" aria-label="Preview state">
          {PREVIEW_STATES.map(([state, label]) => (
            <button
              key={state}
              className={`fchip${previewState === state ? " fchip-on" : ""}`}
              onClick={() => setPreviewState(state)}
            >{label}</button>
          ))}
        </div>
        <InventoryGrid
          items={items.filter(item => previewState === "all" || expired || item.state === previewState)}
          loading={false}
          monitoring
          view={view}
          inventory={{
            "preview-gained": { mastery_rank: 30 },
            "preview-crafting": { mastery_rank: 14 },
          }}
          modCopies={{
            "preview-mod-transfer": [{ rank: 5, count: 2 }],
            "preview-mod-gained": [{ rank: 3, count: 3 }],
          }}
          favorites={favorites}
          lastChanged={{
            "preview-gained": now - (expired ? 301 : 0),
            "preview-lost": now - (expired ? 301 : 0),
            "preview-mod-transfer": now - (expired ? 301 : 0),
            "preview-mod-gained": now - (expired ? 301 : 0),
          }}
          changes={changes}
          crafting={new Map([["preview-crafting", { item_name: "Building item" }]])}
          filterRank={null}
          onToggleFavorite={id => setFavorites(previous => {
            const next = new Set(previous);
            if (next.has(id)) next.delete(id);
            else next.add(id);
            return next;
          })}
        />
      </section>
    </div>
  );
}
