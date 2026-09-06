export type ViewMode = "cards" | "icons" | "text-cards" | "list" | "list-compact";

const VIEW_LABELS: Record<ViewMode, string> = {
  "cards":        "Cards (icon + text)",
  "icons":        "Icon grid",
  "text-cards":   "Text cards (no icons)",
  "list":         "List with icon",
  "list-compact": "Compact list (text only)",
};

function ViewIcon({ mode }: { mode: ViewMode }) {
  switch (mode) {
    case "cards": return (
      <svg width="16" height="13" viewBox="0 0 16 13" fill="currentColor">
        <rect x="0" y="0" width="3" height="3" rx="0.5"/><rect x="4" y="0.5" width="3.5" height="1.5" rx="0.4"/>
        <rect x="9" y="0" width="3" height="3" rx="0.5"/><rect x="13" y="0.5" width="3.5" height="1.5" rx="0.4"/>
        <rect x="0" y="5" width="3" height="3" rx="0.5"/><rect x="4" y="5.5" width="3.5" height="1.5" rx="0.4"/>
        <rect x="9" y="5" width="3" height="3" rx="0.5"/><rect x="13" y="5.5" width="3.5" height="1.5" rx="0.4"/>
        <rect x="0" y="10" width="3" height="3" rx="0.5"/><rect x="4" y="10.5" width="3.5" height="1.5" rx="0.4"/>
        <rect x="9" y="10" width="3" height="3" rx="0.5"/><rect x="13" y="10.5" width="3.5" height="1.5" rx="0.4"/>
      </svg>
    );
    case "icons": return (
      <svg width="16" height="13" viewBox="0 0 16 13" fill="currentColor">
        <rect x="0" y="0" width="4" height="4" rx="0.5"/><rect x="6" y="0" width="4" height="4" rx="0.5"/><rect x="12" y="0" width="4" height="4" rx="0.5"/>
        <rect x="0" y="5" width="4" height="4" rx="0.5"/><rect x="6" y="5" width="4" height="4" rx="0.5"/><rect x="12" y="5" width="4" height="4" rx="0.5"/>
        <rect x="0" y="10" width="4" height="4" rx="0.5"/><rect x="6" y="10" width="4" height="4" rx="0.5"/><rect x="12" y="10" width="4" height="4" rx="0.5"/>
      </svg>
    );
    case "text-cards": return (
      <svg width="16" height="13" viewBox="0 0 16 13" fill="currentColor">
        <rect x="0" y="0" width="7" height="1.5" rx="0.4"/><rect x="0" y="2.5" width="5" height="1" rx="0.4"/>
        <rect x="9" y="0" width="7" height="1.5" rx="0.4"/><rect x="9" y="2.5" width="5" height="1" rx="0.4"/>
        <rect x="0" y="5" width="7" height="1.5" rx="0.4"/><rect x="0" y="7.5" width="5" height="1" rx="0.4"/>
        <rect x="9" y="5" width="7" height="1.5" rx="0.4"/><rect x="9" y="7.5" width="5" height="1" rx="0.4"/>
        <rect x="0" y="10" width="7" height="1.5" rx="0.4"/><rect x="0" y="12" width="5" height="1" rx="0.4"/>
        <rect x="9" y="10" width="7" height="1.5" rx="0.4"/><rect x="9" y="12" width="5" height="1" rx="0.4"/>
      </svg>
    );
    case "list": return (
      <svg width="16" height="13" viewBox="0 0 16 13" fill="currentColor">
        <rect x="0" y="0" width="2.5" height="2.5" rx="0.4"/><rect x="4" y="0.5" width="12" height="1.5" rx="0.4"/>
        <rect x="0" y="3.5" width="2.5" height="2.5" rx="0.4"/><rect x="4" y="4" width="12" height="1.5" rx="0.4"/>
        <rect x="0" y="7" width="2.5" height="2.5" rx="0.4"/><rect x="4" y="7.5" width="12" height="1.5" rx="0.4"/>
        <rect x="0" y="10.5" width="2.5" height="2.5" rx="0.4"/><rect x="4" y="11" width="12" height="1.5" rx="0.4"/>
      </svg>
    );
    case "list-compact": return (
      <svg width="16" height="13" viewBox="0 0 16 13" fill="currentColor">
        <rect x="0" y="0" width="16" height="1.5" rx="0.4"/>
        <rect x="0" y="2.5" width="11" height="1.5" rx="0.4"/>
        <rect x="0" y="5" width="16" height="1.5" rx="0.4"/>
        <rect x="0" y="7.5" width="13" height="1.5" rx="0.4"/>
        <rect x="0" y="10" width="16" height="1.5" rx="0.4"/>
        <rect x="0" y="12" width="10" height="1" rx="0.4"/>
      </svg>
    );
  }
}

export function ViewToggle({ view, onChange }: { view: ViewMode; onChange: (v: ViewMode) => void }) {
  const modes: ViewMode[] = ["cards", "icons", "text-cards", "list", "list-compact"];
  return (
    <div className="view-toggle">
      {modes.map(m => (
        <button key={m} className={`view-btn${view === m ? " view-btn-active" : ""}`}
          title={VIEW_LABELS[m]} onClick={() => onChange(m)}>
          <ViewIcon mode={m} />
        </button>
      ))}
    </div>
  );
}
