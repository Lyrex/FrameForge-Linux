import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { overlayScale } from "../lib/uiScale";
import {
  ensureRivenWindow,
  getRivenLastTriggerMs,
  getRivenRollCount,
  incrementRivenRollCount,
  rivenWinHide,
  resizeRivenForScale,
  setRivenLastTriggerMs,
  setRivenManualTrigger,
} from "../lib/rivenWindow";
import { PREFERENCE_KEYS } from "../constants/preferences";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import type { AddTradeArgs, AnalyzeRivenArgs, InventoryRewardPayload, OcrRivenScreenResult, OverlayWindowBounds, RelicRewardsPayload, WarframeWindowRect } from "../types/tauri";
import type { RivenAnalysis, RivenAnalysisUpdate } from "../types/rivens";
import type { TradeCompletedEvent } from "../types/trades";
import type { QuantityMap } from "../types/items";

interface UseOverlaysReturn {
  overlayStatus: string;
  setQuantities: React.Dispatch<React.SetStateAction<QuantityMap>>;
}

export function useOverlays(
  setQuantities: React.Dispatch<React.SetStateAction<QuantityMap>>,
): UseOverlaysReturn {
  const [overlayStatus, setOverlayStatus] = useState("");

  // ── Riven overlay ─────────────────────────────────────────────────────────
  useEffect(() => {
    const runRivenCheck = async () => {
      setRivenLastTriggerMs(Date.now());
      incrementRivenRollCount();
      const { emit } = await import("@tauri-apps/api/event");

      let rect: WarframeWindowRect = [0, 0, 0, 800];
      try { rect = await invoke<WarframeWindowRect>("get_warframe_window_rect"); } catch {}
      const [wx, wy, , wh] = rect;
      const result = await ensureRivenWindow(wx, wy, wh);
      let pendingPayload: RivenAnalysisUpdate | null = null;
      let windowReady = false;

      if (result && !result.fresh) {
        await emit(TAURI_EVENTS.RIVEN_SCANNING_START, {}).catch(() => {});
        windowReady = true;
      } else if (result?.fresh) {
        const unsubReady = await listen(TAURI_EVENTS.RIVEN_WINDOW_READY, async () => {
          unsubReady();
          windowReady = true;
          if (pendingPayload) { await emit(TAURI_EVENTS.RIVEN_ANALYSIS_UPDATE, pendingPayload).catch(() => {}); pendingPayload = null; }
        });
      }

      try {
        const ocrResult = await invoke<OcrRivenScreenResult>("ocr_riven_screen");
        const analysis: RivenAnalysis | null = (ocrResult.weapon || ocrResult.positives.length > 0)
          ? await invoke<RivenAnalysis | null>(TAURI_COMMANDS.ANALYZE_RIVEN, { weapon: ocrResult.weapon, positives: ocrResult.positives, negatives: ocrResult.negatives } satisfies AnalyzeRivenArgs).catch(() => null)
          : null;
        const payload: RivenAnalysisUpdate = { analysis, ocrRaw: ocrResult.raw, weapon: ocrResult.weapon, positives: ocrResult.positives, negatives: ocrResult.negatives, rolledStats: ocrResult.rolled_stats, isComparison: ocrResult.is_comparison, originalStats: ocrResult.original_rolled_stats, rollCount: getRivenRollCount() };
        if (windowReady) { await emit(TAURI_EVENTS.RIVEN_ANALYSIS_UPDATE, payload).catch(() => {}); }
        else              { pendingPayload = payload; }
      } catch (e) {
        await invoke("ocr_riven_log_error", { error: String(e) }).catch(() => {});
        const payload: RivenAnalysisUpdate = { analysis: null, ocrRaw: `OCR ERROR: ${e}`, weapon: "", positives: [], negatives: [], rolledStats: [], isComparison: false, originalStats: [], rollCount: getRivenRollCount() };
        if (windowReady) { await emit(TAURI_EVENTS.RIVEN_ANALYSIS_UPDATE, payload).catch(() => {}); }
        else              { pendingPayload = payload; }
      }
    };

    setRivenManualTrigger(() => { runRivenCheck().catch(() => {}); });

    const unsubManual = listen(TAURI_EVENTS.RIVEN_MANUAL_CHECK, () => runRivenCheck().catch(() => {}));

    const triggerOpen = () => {
      const now = Date.now();
      if (now - getRivenLastTriggerMs() < 4000) return;
      runRivenCheck().catch(() => {});
    };
    const unsubAutoDetect = listen("riven-screen-open", () => triggerOpen());

    const unsubClose   = listen("riven-screen-close",   () => rivenWinHide("screen-close"));
    const unsubHideReq = listen<{ reason?: string }>(TAURI_EVENTS.RIVEN_OVERLAY_HIDE, e => rivenWinHide(e.payload?.reason ?? "overlay-hide"));
    const unsubSettings = listen(TAURI_EVENTS.SETTINGS_UPDATED, () => { void resizeRivenForScale(); });

    return () => {
      unsubManual.then(fn => fn());
      unsubAutoDetect.then(fn => fn());
      unsubClose.then(fn => fn());
      unsubHideReq.then(fn => fn());
      unsubSettings.then(fn => fn());
      setRivenManualTrigger(null);
    };
  }, []); // eslint-disable-line

  // ── Relic reward overlay ──────────────────────────────────────────────────
  useEffect(() => {
    let overlayVisible = false;

    const closeOverlay = async () => {
      overlayVisible = false;
      await invoke(TAURI_COMMANDS.MOVE_OVERLAY_OFFSCREEN).catch(() => {});
    };

    const unsubStatus = listen<string>("ff-status", (e) => {
      setOverlayStatus(e.payload);
      setTimeout(() => setOverlayStatus(""), 4000);
    });

    const openOverlay = async (
      wx: number, wy: number, ww: number, wh: number,
      yFrac: number, hFrac: number,
    ): Promise<boolean> => {
      const offsetY = Math.round(wh * yFrac);
      const stripH  = Math.min(Math.round(wh * hFrac * overlayScale()), wh - offsetY);
      const stripY  = wy + offsetY;
      try {
        const bounds: OverlayWindowBounds = { x: wx, y: stripY, w: ww, h: stripH };
        await invoke("show_overlay_window", bounds);
        overlayVisible = true;
        return true;
      } catch { return false; }
    };

    const unsubTrigger = listen<null>(TAURI_EVENTS.RELIC_TRIGGER, async () => {
      const enabled = localStorage.getItem(PREFERENCE_KEYS.OVERLAY_ENABLED) !== "false";
      if (!enabled) return;
      try {
        const [wx, wy, ww, wh] = await invoke<WarframeWindowRect>("get_warframe_window_rect");
        invoke(TAURI_COMMANDS.LOG_RELIC_FE, { msg: `[APP] relic-trigger: wf(${wx},${wy} ${ww}×${wh})` }).catch(() => {});
        await openOverlay(wx, wy, ww, wh, 0.60, 0.30);
      } catch (e) {
        invoke(TAURI_COMMANDS.LOG_RELIC_FE, { msg: `[APP] relic-trigger: wf-rect failed (${e}), falling back to screen dims` }).catch(() => {});
        const sw = window.screen.width, sh = window.screen.height;
        await openOverlay(0, 0, sw, sh, 0.60, 0.30);
      }
    });

    const unsubRelic = listen<boolean>(TAURI_EVENTS.RELIC_SCREEN, () => { closeOverlay(); });

    const unsub = listen<RelicRewardsPayload | null>(TAURI_EVENTS.RELIC_REWARDS, async (e) => {
      const rewards = e.payload;
      if (!rewards || rewards.items.length === 0) { closeOverlay(); return; }
      const enabled = localStorage.getItem(PREFERENCE_KEYS.OVERLAY_ENABLED) !== "false";
      if (!enabled) return;
      invoke(TAURI_COMMANDS.LOG_RELIC_FE, { msg: `[APP] relic-rewards: ${rewards.items.length} items, overlayVisible=${overlayVisible}` }).catch(() => {});
      if (!overlayVisible) {
        try {
          const [wx, wy, ww, wh] = await invoke<WarframeWindowRect>("get_warframe_window_rect");
          invoke(TAURI_COMMANDS.LOG_RELIC_FE, { msg: `[APP] relic-rewards fallback: wf(${wx},${wy} ${ww}×${wh})` }).catch(() => {});
          await openOverlay(wx, wy, ww, wh, 0.54, 0.28);
        } catch (err) {
          invoke(TAURI_COMMANDS.LOG_RELIC_FE, { msg: `[APP] relic-rewards fallback: wf-rect failed (${err}), using screen dims` }).catch(() => {});
          const sw = window.screen.width, sh = window.screen.height;
          await openOverlay(0, 0, sw, sh, 0.54, 0.28);
        }
      }
    });

    const unsubReward = listen<InventoryRewardPayload>("inventory-reward", (e) => {
      const { path, qty } = e.payload;
      setQuantities(prev => ({ ...prev, [path]: qty }));
    });

    return () => {
      unsub.then(fn => fn());
      unsubRelic.then(fn => fn());
      unsubTrigger.then(fn => fn());
      unsubStatus.then(fn => fn());
      unsubReward.then(fn => fn());
      invoke(TAURI_COMMANDS.MOVE_OVERLAY_OFFSCREEN).catch(() => {});
    };
  }, [setQuantities]);

  // ── In-game trade detection ───────────────────────────────────────────────
  useEffect(() => {
    const unlisten = listen<TradeCompletedEvent>(TAURI_EVENTS.TRADE_COMPLETED, async (e) => {
      const p = e.payload;
      const save = (dir: string, name: string, qty: number, plat: number) => {
        const args: AddTradeArgs = {
          withPlayer: p.withPlayer,
          direction:  dir,
          itemName:   name,
          itemUrl:    "",
          quantity:   qty,
          platinum:   plat,
          source:     "in-game",
          notes:      "",
          sessionId:  p.sessionId,
          tradeType:  p.tradeType,
          timestamp:  p.timestamp,
        };
        return invoke(TAURI_COMMANDS.ADD_TRADE, args).catch(() => {});
      };

      if (p.tradeType === "sale") {
        for (let i = 0; i < p.offeredItems.length; i++) {
          const item = p.offeredItems[i];
          await save("sold", item.name, item.qty, i === 0 ? p.receivedPlat : 0);
        }
      } else if (p.tradeType === "purchase") {
        for (let i = 0; i < p.receivedItems.length; i++) {
          const item = p.receivedItems[i];
          await save("bought", item.name, item.qty, i === 0 ? p.offeredPlat : 0);
        }
      } else {
        for (const item of p.offeredItems)  await save("traded-out", item.name, item.qty, 0);
        for (const item of p.receivedItems) await save("traded-in",  item.name, item.qty, 0);
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  return {
    overlayStatus,
    setQuantities,
  };
}
