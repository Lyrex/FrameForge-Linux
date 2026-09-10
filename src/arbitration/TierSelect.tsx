import { useEffect, useRef, useState } from "react";
import { TIER_KEYS, TIER_LABELS, type TierKey } from "./arbitrationTiers";

type Props = {
  label: string;
  selected: readonly TierKey[];
  onChange: (next: TierKey[]) => void;
};

/// Reads as a sentence rather than a list of six once the selection is wide:
/// "All" and "None" are what the user set, not something to spell out.
function summarize(selected: readonly TierKey[]): string {
  if (selected.length === 0) return "None";
  if (selected.length === TIER_KEYS.length) return "All";
  return TIER_KEYS.filter((k) => selected.includes(k))
    .map((k) => TIER_LABELS[k])
    .join(", ");
}

/// Multi-select over the arbitration tiers, shared by the schedule's filter
/// and its alert rule. A native `<select multiple>` would need a modifier key
/// held to pick a second tier, which is not discoverable for the one control
/// most users will touch here.
export default function TierSelect({ label, selected, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  // A click anywhere else is a dismissal, including on the other picker: two
  // popovers open at once would overlap.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const toggle = (key: TierKey) =>
    onChange(
      TIER_KEYS.filter((k) =>
        k === key ? !selected.includes(k) : selected.includes(k),
      ),
    );

  return (
    <div className="tier-select" ref={rootRef}>
      <span className="tier-select-label">{label}</span>
      <button
        type="button"
        className={`tier-select-btn${open ? " open" : ""}`}
        aria-expanded={open}
        aria-label={label}
        onClick={() => setOpen((o) => !o)}
      >
        {summarize(selected)}
        <span className="tier-select-caret">▾</span>
      </button>
      {open && (
        <div className="tier-select-menu">
          {TIER_KEYS.map((k) => (
            <label key={k} className="tier-select-row">
              <input
                type="checkbox"
                checked={selected.includes(k)}
                onChange={() => toggle(k)}
              />
              <TierBadge tier={k === "unrated" ? null : k} showUnrated />
              <span>{TIER_LABELS[k]}</span>
            </label>
          ))}
          <div className="tier-select-actions">
            <button type="button" onClick={() => onChange([...TIER_KEYS])}>
              All
            </button>
            <button type="button" onClick={() => onChange([])}>
              None
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/// The letter carries the rating on its own, so the color is decoration and
/// the badge stays readable under the colorblind palette.
export function TierBadge({
  tier,
  showUnrated = false,
}: {
  tier: TierKey | null;
  showUnrated?: boolean;
}) {
  // An unrated node keeps the badge's width so the node names below each
  // other still line up; only the dropdown spells the absence out.
  if (tier === null || tier === "unrated") {
    return (
      <span className={`tier-badge tier-${showUnrated ? "unrated" : "none"}`}>
        {showUnrated ? "–" : ""}
      </span>
    );
  }
  return (
    <span className={`tier-badge tier-${tier}`} title={`Tier ${tier}`}>
      {tier}
    </span>
  );
}
