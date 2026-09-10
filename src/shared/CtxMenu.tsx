import { useState, useCallback, useRef } from "react";
import { appScale } from "../lib/uiScale";
import { useClickOutside } from "./useClickOutside";

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

export function CtxMenu({ state, onClose }: { state: CtxMenuState; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useClickOutside(ref, onClose);
  return (
    <div ref={ref} className="ctx-menu" style={{ left: state.x, top: state.y }}>
      {state.items.map((item, i) => (
        <span key={i}>
          {i > 0 && <div className="ctx-menu-sep" />}
          <button className="ctx-menu-item" onClick={() => { item.action(); onClose(); }}>
            {item.label}
          </button>
        </span>
      ))}
    </div>
  );
}
