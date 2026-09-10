import { useState, useRef } from "react";
import { useClickOutside } from "./useClickOutside";

export interface HelpItem {
  swatch?: string;   // CSS color string for a colored square
  border?: string;   // CSS color for a border-top sample
  icon?: string;     // emoji or text icon
  label: string;
  desc: string;
}

export function HelpTip({ items, align = "right" }: { items: HelpItem[]; align?: "left" | "right" }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useClickOutside(ref, () => setOpen(false), open);

  return (
    <div ref={ref} style={{ position: "relative", flexShrink: 0 }}>
      <button className="helptip-btn" onClick={() => setOpen(v => !v)} title="Color legend">?</button>
      {open && (
        <div className="helptip-popup" style={{ [align === "left" ? "left" : "right"]: 0 }}>
          <div className="helptip-title">Legend</div>
          {items.map((item, i) => (
            <div key={i} className="helptip-row">
              {item.swatch && (
                <span className="helptip-swatch" style={{ background: item.swatch }} />
              )}
              {item.border && (
                <span className="helptip-border-sample" style={{ borderTopColor: item.border }} />
              )}
              {item.icon && <span className="helptip-icon">{item.icon}</span>}
              <div>
                <span className="helptip-label">{item.label}</span>
                <span className="helptip-desc">{item.desc}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
