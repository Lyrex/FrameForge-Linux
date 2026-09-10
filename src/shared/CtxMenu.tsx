import { useState, useEffect, useCallback } from "react";
import { appScale } from "../lib/uiScale";

interface CtxMenuItem {
  label: string;
  action: () => void;
}
interface CtxMenuState {
  x: number;
  y: number;
  items: CtxMenuItem[];
}

export function useContextMenu() {
  const [ctxMenu, setCtxMenu] = useState<CtxMenuState | null>(null);

  useEffect(() => {
    if (!ctxMenu) return;
    const close = () => setCtxMenu(null);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", onKey);
    };
  }, [ctxMenu]);

  // The menu is fixed-positioned inside #root, which is zoomed by the text
  // scale, so its coordinates are in zoomed units while the mouse event and
  // the viewport size are in real pixels.
  const open = useCallback((clientX: number, clientY: number, items: CtxMenuItem[]) => {
    const scale = appScale();
    const menuW = 180;
    const menuH = items.length * 30 + 8;
    const maxX = window.innerWidth / scale - menuW;
    const maxY = window.innerHeight / scale - menuH;
    setCtxMenu({ x: Math.min(clientX / scale, maxX), y: Math.min(clientY / scale, maxY), items });
  }, []);

  const close = useCallback(() => setCtxMenu(null), []);

  return { ctxMenu, open, close };
}

const menuStyle: React.CSSProperties = {
  position: "fixed",
  zIndex: 999,
  background: "var(--surface)",
  border: "1px solid var(--border)",
  borderRadius: 8,
  minWidth: 160,
  boxShadow: "0 4px 16px rgba(0,0,0,.5)",
};

const itemStyle: React.CSSProperties = {
  display: "block",
  width: "100%",
  textAlign: "left",
  background: "none",
  border: "none",
  padding: "6px 14px",
  color: "var(--text)",
  cursor: "default",
  whiteSpace: "nowrap",
};

const menuHoverStyle: React.CSSProperties = {
  borderColor: "rgba(56,139,253,.5)",
};

const itemHoverStyle: React.CSSProperties = {
  background: "rgba(56,139,253,.15)",
};

const sepStyle: React.CSSProperties = {
  height: 1,
  background: "var(--border)",
};

export function CtxMenu({ state, onClose }: { state: CtxMenuState; onClose: () => void }) {
  const [hovered, setHovered] = useState(false);
  return (
    <div style={{ ...menuStyle, ...(hovered ? menuHoverStyle : {}), left: state.x, top: state.y }}
         onMouseDown={e => e.stopPropagation()}
         onMouseEnter={() => setHovered(true)}
         onMouseLeave={() => setHovered(false)}>
      {state.items.map((item, i) => (
        <span key={i}>
          {i > 0 && <div style={sepStyle} />}
          <HoverItem onClick={() => { item.action(); onClose(); }}>
            {item.label}
          </HoverItem>
        </span>
      ))}
    </div>
  );
}

function HoverItem({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  const [hovered, setHovered] = useState(false);
  return (
    <button style={hovered ? { ...itemStyle, ...itemHoverStyle } : itemStyle}
            onMouseEnter={() => setHovered(true)}
            onMouseLeave={() => setHovered(false)}
            onClick={onClick}>
      {children}
    </button>
  );
}
