import { useState, useEffect, useRef } from "react";

export interface HelpItem {
  swatch?: string;   // CSS color string for a colored square
  border?: string;   // CSS color for a border-top sample
  icon?: string;     // emoji or text icon
  label: string;
  desc: string;
}

const HT_BTN =
  "w-[18px] h-[18px] rounded-full border border-border bg-[rgba(255,255,255,.04)] text-muted text-[10px] font-bold leading-none cursor-pointer inline-flex items-center justify-center shrink-0 transition-[color,border-color] duration-[120ms] hover:text-foreground hover:border-accent";
const HT_POPUP =
  "absolute top-[26px] bg-[#1a1f2a] border border-border rounded-[8px] px-3 py-[10px] z-[600] min-w-[230px] max-w-[300px] max-h-[calc(60vh_/_var(--ff-scale,1))] overflow-y-auto shadow-[0_8px_28px_rgba(0,0,0,.7)] flex flex-col gap-[7px]";
const HT_TITLE =
  "text-[10px] font-bold uppercase tracking-[.07em] text-muted mb-[2px]";
const HT_ROW = "flex items-start gap-2";
const HT_SWATCH = "w-[13px] h-[13px] rounded-[3px] shrink-0 mt-px";
const HT_BORDER_SAMPLE =
  "w-[13px] h-[13px] shrink-0 mt-px rounded-[2px] bg-[rgba(255,255,255,.04)] border border-border border-t-[3px]";
const HT_ICON = "text-[13px] w-4 text-center shrink-0 leading-[1.4]";
const HT_LABEL = "text-[11px] font-semibold text-foreground block";
const HT_DESC = "text-[10px] text-muted block leading-[1.4]";

export function HelpTip({ items, align = "right" }: { items: HelpItem[]; align?: "left" | "right" }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  return (
    <div ref={ref} className="relative shrink-0">
      <button className={HT_BTN} onClick={() => setOpen(v => !v)} title="Color legend">?</button>
      {open && (
        <div className={`${HT_POPUP} ${align === "left" ? "left-0" : "right-0"}`}>
          <div className={HT_TITLE}>Legend</div>
          {items.map((item, i) => (
            <div key={i} className={HT_ROW}>
              {item.swatch && (
                <span className={HT_SWATCH} style={{ background: item.swatch }} />
              )}
              {item.border && (
                <span className={HT_BORDER_SAMPLE} style={{ borderTopColor: item.border }} />
              )}
              {item.icon && <span className={HT_ICON}>{item.icon}</span>}
              <div>
                <span className={HT_LABEL}>{item.label}</span>
                <span className={HT_DESC}>{item.desc}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
