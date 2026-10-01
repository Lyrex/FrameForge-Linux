import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { LogicalPosition, LogicalSize, availableMonitors } from "@tauri-apps/api/window";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import { ensureRivenWindow, rivenPlacement, rivenWinHide } from "./rivenWindow";
import { overlayScale } from "./uiScale";

type Rect = [number, number, number, number];

/** Game window rect in physical pixels; primary monitor when Warframe is not running. */
async function rectOrScreen(): Promise<Rect> {
  try {
    return await invoke<Rect>("get_warframe_window_rect");
  } catch {
    try {
      const m = (await availableMonitors())[0];
      if (m) return [m.position.x, m.position.y, m.size.width, m.size.height];
    } catch {}
    return [0, 0, 1920, 1080];
  }
}

const send = (target: string) => emit(TAURI_EVENTS.OVERLAY_OUTLINE, target).catch(() => {});

/** Reward strip: same placement path as a real relic-trigger, then draw the outline. */
export async function showRewardOutline(): Promise<void> {
  const [wx, wy, ww, wh] = await rectOrScreen();
  const offsetY = Math.round(wh * 0.60);
  const stripH = Math.min(Math.round(wh * 0.30 * overlayScale()), wh - offsetY);
  try {
    await invoke("show_overlay_window", { x: wx, y: wy + offsetY, w: ww, h: stripH });
    await send("relic");
  } catch {}
}

export async function hideRewardOutline(): Promise<void> {
  await send("off-relic");
  await invoke(TAURI_COMMANDS.MOVE_OVERLAY_OFFSCREEN).catch(() => {});
}

/** Relic pick window: show it at its normal spot, then draw the outline. */
export async function showPickOutline(): Promise<void> {
  try {
    await invoke("show_relic_pick_window");
  } catch {
    return;
  }
  await send("relicPick");
}

export async function hidePickOutline(): Promise<void> {
  await send("off-relicPick");
  await invoke("hide_relic_pick_overlay").catch(() => {});
}

/** Riven overlay: show it where the current offsets put it (no outline mode). */
export async function showRivenOverlay(): Promise<void> {
  const [wx, wy, , wh] = await rectOrScreen();
  const result = await ensureRivenWindow(wx, wy, wh);
  if (!result) return;
  if (!result.fresh) {
    // Already open — re-place it so the show reflects the saved offsets.
    try {
      const p = await rivenPlacement(wx, wy, wh);
      await result.win.setPosition(new LogicalPosition(p.x, p.y));
      await result.win.setSize(new LogicalSize(p.width, p.height));
    } catch {}
  }
}

export async function hideRivenOverlay(): Promise<void> {
  rivenWinHide("manual-hide", false);
}
