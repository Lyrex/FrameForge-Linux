import { useState, useEffect, useCallback, useRef } from "react";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { availableMonitors } from "@tauri-apps/api/window";
import { MODULAR_SECTION_ORDER_DEFAULT } from "../constants/settings";
import type { SettingsFile } from "../types/tauri";

const sameArray = <T,>(left: T[], right: T[]) =>
  left.length === right.length && left.every((value, index) => value === right[index]);

interface UseModularWindowReturn {
  // State
  tracked: string[];
  favorites: string[];
  modularWidth: number;
  modularSectionOrder: string[];
  modularPopout: boolean;

  // Refs
  modularWinRef: React.MutableRefObject<WebviewWindow | null>;
  modularWinGeomRef: React.MutableRefObject<{ x?: number; y?: number; w?: number; h?: number }>;

  // Callbacks
  toggleTracked: (id: string) => void;
  toggleFavorite: (id: string) => void;
  applySettings: (settings: SettingsFile) => void;

  // Setters
  setTracked: React.Dispatch<React.SetStateAction<string[]>>;
  setFavorites: React.Dispatch<React.SetStateAction<string[]>>;
  setModularWidth: React.Dispatch<React.SetStateAction<number>>;
  setModularSectionOrder: React.Dispatch<React.SetStateAction<string[]>>;
  setModularPopout: React.Dispatch<React.SetStateAction<boolean>>;
}

export function useModularWindow(): UseModularWindowReturn {
  // ── Modular Window state ──────────────────────────────────────────────────
  const [tracked, setTracked] = useState<string[]>([]);
  const [favorites, setFavorites] = useState<string[]>([]);
  const [modularWidth, setModularWidth] = useState(240);
  const [modularSectionOrder, setModularSectionOrder] = useState<string[]>([...MODULAR_SECTION_ORDER_DEFAULT]);
  const [modularPopout, setModularPopout] = useState(false);
  const modularWinRef = useRef<WebviewWindow | null>(null);
  const modularWinGeomRef = useRef<{ x?: number; y?: number; w?: number; h?: number }>({});

  // ── Toggle callbacks ──────────────────────────────────────────────────────
  const toggleTracked = useCallback((id: string) => {
    setTracked(prev => prev.includes(id) ? prev.filter(i => i !== id) : [...prev, id]);
  }, []);

  const toggleFavorite = useCallback((id: string) => {
    setFavorites(prev => prev.includes(id) ? prev.filter(i => i !== id) : [...prev, id]);
  }, []);

  const applySettings = useCallback((settings: SettingsFile) => {
    if (Array.isArray(settings.tracked)) {
      setTracked(current => sameArray(current, settings.tracked) ? current : settings.tracked);
    }
    if (Array.isArray(settings.favorites)) {
      setFavorites(current => sameArray(current, settings.favorites) ? current : settings.favorites);
    }
    if (typeof settings.modularWidth === "number") {
      setModularWidth(current => current === settings.modularWidth ? current : settings.modularWidth);
    }
    if (Array.isArray(settings.modularSectionOrder)) {
      const order = [...settings.modularSectionOrder];
      if (!order.includes("timers")) order.push("timers");
      if (!order.includes("fissures")) order.push("fissures");
      setModularSectionOrder(current => sameArray(current, order) ? current : order);
    }
    if (typeof settings.modularPopout === "boolean") {
      setModularPopout(current => current === settings.modularPopout ? current : settings.modularPopout);
    }
    if (typeof settings.modularWinX === "number") modularWinGeomRef.current.x = settings.modularWinX;
    if (typeof settings.modularWinY === "number") modularWinGeomRef.current.y = settings.modularWinY;
    if (typeof settings.modularWinWidth === "number") modularWinGeomRef.current.w = settings.modularWinWidth;
    if (typeof settings.modularWinHeight === "number") modularWinGeomRef.current.h = settings.modularWinHeight;
  }, []);

  // ── Modular pop-out window ────────────────────────────────────────────────
  useEffect(() => {
    if (modularPopout) {
      if (modularWinRef.current) return;
      const g = modularWinGeomRef.current;

      const createWin = (usePos: boolean) => new WebviewWindow("modular-popout", {
        url: "index.html#modular",
        title: "FrameForge — Modular Window",
        width: g.w ?? modularWidth,
        height: g.h ?? 700,
        ...(usePos && g.x !== undefined ? { x: g.x } : {}),
        ...(usePos && g.y !== undefined ? { y: g.y } : {}),
        minWidth: 180,
        minHeight: 300,
        resizable: true,
        decorations: true,
        alwaysOnTop: false,
      });

      let win: WebviewWindow;
      if (g.x !== undefined && g.y !== undefined) {
        availableMonitors().then(monitors => {
          const onScreen = monitors.some(m => {
            const mp = m.position; const ms = m.size;
            return g.x! >= mp.x && g.x! < mp.x + ms.width &&
                   g.y! >= mp.y && g.y! < mp.y + ms.height;
          });
          win = createWin(onScreen);
          modularWinRef.current = win;
          win.once("tauri://destroyed", () => { modularWinRef.current = null; setModularPopout(false); });
        }).catch(() => {
          win = createWin(false);
          modularWinRef.current = win;
          win.once("tauri://destroyed", () => { modularWinRef.current = null; setModularPopout(false); });
        });
        return;
      }
      win = createWin(false);
      modularWinRef.current = win;
      win.once("tauri://destroyed", () => {
        modularWinRef.current = null;
        setModularPopout(false);
      });
    } else {
      modularWinRef.current?.close().catch(() => {});
      modularWinRef.current = null;
    }
  }, [modularPopout]); // eslint-disable-line

  return {
    // State
    tracked,
    favorites,
    modularWidth,
    modularSectionOrder,
    modularPopout,

    // Refs
    modularWinRef,
    modularWinGeomRef,

    // Callbacks
    toggleTracked,
    toggleFavorite,
    applySettings,

    // Setters
    setTracked,
    setFavorites,
    setModularWidth,
    setModularSectionOrder,
    setModularPopout,
  };
}
