import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { applyScale, overlayScale } from "./lib/uiScale";
import { useContextMenu, CtxMenu } from "./shared/CtxMenu";
import { extractItemName } from "./lib/itemContext";
import { openWiki, copyWikiLink } from "./lib/wiki";

// ── Riven overlay — module-level window management ────────────────────────────
// Stored OUTSIDE React so StrictMode remounts don't destroy/recreate the window.
let _rivenWin: WebviewWindow | null = null;
let _rivenRollCount = 0;
let _rivenLastTriggerMs = 0;
let _rivenManualTrigger: (() => void) | null = null;
export function checkRivenNow() { _rivenManualTrigger?.(); }

async function resizeRivenForScale() {
  const win = _rivenWin;
  if (!win) return;
  try {
    const factor = await win.scaleFactor();
    const cur = (await win.innerSize()).toLogical(factor);
    await win.setSize(new LogicalSize(Math.round(300 * overlayScale()), cur.height));
  } catch {}
}

function rivenWinHide(reason = "rivenWinHide") {
  const win = _rivenWin;
  if (!win) { return; }
  invoke("ocr_riven_log_error", { error: `[HIDE] ${reason}` }).catch(() => {});
  _rivenWin = null;
  win.close().catch(() => {});
}

async function ensureRivenWindow(wx: number, wy: number, wh: number): Promise<{ win: WebviewWindow; fresh: boolean } | null> {
  // 1. Existing valid handle
  if (_rivenWin) return { win: _rivenWin, fresh: false };

  // 2. Window exists but JS lost reference (HMR, page reload)
  const existing = await WebviewWindow.getByLabel("riven-overlay").catch(() => null);
  if (existing) {
    _rivenWin = existing;
    _rivenWin.once("tauri://destroyed", () => { _rivenWin = null; });
    return { win: _rivenWin, fresh: false };
  }

  // 3. Create fresh at correct position — shows immediately
  try {
    _rivenWin = new WebviewWindow("riven-overlay", {
      url: `index.html#rivenoverlay`,
      title: "FrameForge Riven",
      transparent: true, decorations: false,
      alwaysOnTop: true, skipTaskbar: true,
      resizable: false, focus: false,
      x: wx + 10, y: wy + Math.round(wh * 0.20),
      width: Math.round(300 * overlayScale()), height: Math.round(wh * 0.60),
    });
    _rivenWin.once("tauri://destroyed", () => { _rivenWin = null; });
    return { win: _rivenWin, fresh: true };
  } catch {
    _rivenWin = null;
    return null;
  }
}
import { getCurrentWindow, availableMonitors, LogicalSize } from "@tauri-apps/api/window";

import { ImgCacheDirContext } from "./ImgCacheDir";
import Foundry from "./Foundry";
import CacheStatusChip from "./header/CacheStatusChip";
import MarketHelper from "./market/MarketHelper";
import RelicHelper from "./RelicHelper";
import RivenAnalyzer from "./riven/RivenAnalyzer";
import RivenOverlayWindow from "./riven/RivenOverlayWindow";
import RelicPickOverlay from "./relic-overlay/RelicPickOverlay";
import ArbitrationOverlay from "./arbitration/ArbitrationOverlay";
import Arbitrations from "./arbitration/Arbitrations";
import TimerHelper, { fmtMs } from "./TimerHelper";
import { useWorldState } from "./worldstate";
import { notify } from "./lib/notify";
import { collectNewMatches } from "./fissureAlerts";
import { clampLead, runAlertPass, DEFAULT_LEAD_MINS, EVAL_INTERVAL_MS, type AlertRule, type ScheduleEntry } from "./arbitration/arbitrationAlerts";
import { sanitizeTierKeys, TIER_KEYS, type TierKey } from "./arbitration/arbitrationTiers";
import { clampScheduleDays, DEFAULT_SCHEDULE_DAYS, useArbitrationSchedule } from "./arbitration/arbitrationSchedule";
import UpdateDialog from "./update/UpdateDialog";
import { onUpdateAvailable, pendingUpdate, type UpdateAvailable } from "./update/updater";
import Statistics from "./statistics/Statistics";
import Overlay from "./relic-overlay/Overlay";
import ModularWindow from "./modular-window/ModularWindow";
import ModularWindowPage from "./modular-window/ModularWindowPage";
import SettingsModal from "./SettingsModal";
import ChangeLog from "./ChangeLog";
import InventoryGrid from "./inventory/InventoryGrid";
import InventoryBatchPreview from "./inventory/InventoryBatchPreview";
import InventoryToolbar from "./inventory/InventoryToolbar";
import AppNavigation, { type Module } from "./AppNavigation";
import InventorySidebar from "./inventory/InventorySidebar";
import CompletionistTabs from "./completionist/CompletionistTabs";
import HeaderActions from "./header/HeaderActions";
import ErrorBoundary from "./shared/ErrorBoundary";
import HeaderStatusBadges from "./header/HeaderStatusBadges";
import ConnectionStatusChip from "./header/ConnectionStatusChip";
import { INVENTORY_FILTERS_DEFAULT } from "./constants/filters";
import { PREFERENCE_KEYS } from "./constants/preferences";
import {
  CLOCK_FORMAT_OPTIONS,
  DEFAULT_CLOCK_FORMAT,
  DEFAULT_FOUNDRY_PAGE_SIZE,
  DEFAULT_RELIC_OVERLAY_PRIORITY,
  DEFAULT_RELIC_PICK_LINES,
  DEFAULT_RELIC_PICK_PRIORITY,
  DEFAULT_RELIC_PICK_REFINEMENT,
  FOUNDRY_PAGE_SIZE_OPTIONS,
  MODULAR_SECTION_ORDER_DEFAULT,
  RELIC_PICK_LINES_OPTIONS,
  RELIC_PICK_PRIORITY_OPTIONS,
  RELIC_PICK_REFINEMENT_OPTIONS,
} from "./constants/settings";
import { TAURI_COMMANDS, TAURI_EVENTS } from "./constants/tauri";
import type { InventoryFilters } from "./types/filters";
import type { ViewMode } from "./types/ui";
import type { ArchonShard, CatalogItem, CraftingJob, InventoryItem, QuantityMap } from "./types/items";
import type { ChangeLogEntry, InventoryUpdate, ModCopy } from "./types/inventory";
import type { RivenAnalysis, RivenAnalysisUpdate } from "./types/rivens";
import type { ClockFormat } from "./lib/clockFormat";
import type { FissureWatch, FoundryPageSize, RelicOverlayPriority, RelicPickLines, RelicPickPriority, RelicRefinement, SettingsSnapshot } from "./types/settings";
import type { SeenFissures } from "./types/worldstate";
import type { TradeCompletedEvent } from "./types/trades";
import type { AddTradeArgs, AnalyzeRivenArgs, BlobStatusPayload, InventoryRewardPayload, ItemListStatus, OcrRivenScreenResult, OverlayWindowBounds, RelicRewardsPayload, SettingsFile, SettingsPatch, WarframeWindowRect, WfmCredentials, WfmSession } from "./types/tauri";
import "./App.css";
import "./images.css";

const _winLabel = getCurrentWindow().label;
// Support all URL formats: query string (?overlay), hash (#overlay), or window label.
// v2.0.0 used query strings and they worked fine — keep as primary detection path.
const _params          = new URLSearchParams(window.location.search);
const _hash            = window.location.hash;
const IS_OVERLAY       = _params.has("overlay")      || _hash === "#overlay"      || _winLabel === "relic-overlay";
const IS_MODULAR       = _params.has("modular")      || _hash === "#modular"      || _winLabel === "modular-popout";
const IS_RIVEN_OVERLAY      = _params.has("rivenoverlay")      || _hash === "#rivenoverlay"      || _winLabel === "riven-overlay";
const IS_RELIC_PICK_OVERLAY = _params.has("relicpickoverlay") || _hash === "#relicpickoverlay" || _winLabel === "relic-pick-overlay";
const IS_ARBITRATION_OVERLAY = _params.has("arbitrationoverlay") || _hash === "#arbitrationoverlay" || _winLabel === "arbitration-overlay";
const IS_ANY_OVERLAY = IS_OVERLAY || IS_MODULAR || IS_RIVEN_OVERLAY || IS_RELIC_PICK_OVERLAY || IS_ARBITRATION_OVERLAY;

// Overlay windows return from the router before any hook can run, which rules
// out applying the scale from an effect.
applyScale(IS_ANY_OVERLAY);
listen(TAURI_EVENTS.SETTINGS_UPDATED, () => applyScale(IS_ANY_OVERLAY));

// ─── Constants ────────────────────────────────────────────────────────────────

const CATEGORIES = [
  { id: "all",        label: "All Owned" },
  { id: "Resources",  label: "Resources" },
  { id: "Mods",       label: "Mods" },
  { id: "Relics",     label: "Relics" },
  { id: "Arcanes",    label: "Arcanes" },
  { id: "Warframes",  label: "Warframes" },
  { id: "Primary",    label: "Primary" },
  { id: "Secondary",  label: "Secondary" },
  { id: "Melee",      label: "Melee" },
  { id: "Companions",       label: "Companions" },
  { id: "Archwing",         label: "Archwing" },
  { id: "Operator Weapons", label: "Operator Weapons" },
  { id: "Parts",            label: "Parts" },
  { id: "Blueprints", label: "Blueprints" },
  { id: "Miscellaneous", label: "Miscellaneous" },
  { id: "Sigils",     label: "Sigils" },
  { id: "Glyphs",     label: "Glyphs" },
  { id: "Skins",      label: "Skins" },
  { id: "Railjack",   label: "Railjack" },
];

// ─── App ──────────────────────────────────────────────────────────────────────

// RelicAndRivenTab is kept but now just shows RelicHelper — Rivens moved to own tab

export default function App() {
  // If we're the overlay window, render only the overlay UI
  if (IS_OVERLAY) return <Overlay />;
  if (IS_RIVEN_OVERLAY) return <RivenOverlayWindow />;
  if (IS_RELIC_PICK_OVERLAY) return <RelicPickOverlay />;
  if (IS_ARBITRATION_OVERLAY) return <ArbitrationOverlay />;
  // If we're the pop-out modular window, render the standalone modular UI
  if (IS_MODULAR) return <ModularWindowPage />;

  const [activeModule, setActiveModule] = useState<Module>("inventory");
  const { ctxMenu, open: openCtx, close: closeCtx } = useContextMenu();

  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  const [quantities, setQuantities] = useState<Record<string, number>>({});
  const [scannerMods, setScannerMods] = useState<Record<string, { total: number; by_rank: Record<string, number> }>>({});
  const [crafting, setCrafting] = useState<CraftingJob[]>([]);
  const [masteryRank, setMasteryRank] = useState<number | null>(null);
  const [masteryData, setMasteryData] = useState<Record<string, number>>({});
  const [ownedLevels, setOwnedLevels] = useState<Record<string, number[]>>({});
  const [playerName, setPlayerName] = useState<string | null>(null);
  const [poking, setPoking] = useState(false);
  const [autoDiagEnabled, setAutoDiagEnabled] = useState(false);
  const [memoryScannerEnabled, setMemoryScannerEnabled] = useState(false);
const [blobLogEnabled, setBlobLogEnabled] = useState(false);
  const [wfmLoggedIn, setWfmLoggedIn] = useState(false);
  const [wfmInvisibleOnStart,   setWfmInvisibleOnStart]   = useState(false);
  const [wfmInvisibleOnClose,   setWfmInvisibleOnClose]   = useState(false);
  const [wfmAutoInvisible,      setWfmAutoInvisible]      = useState(false);
  const [wfmAutoInvisibleMins,  setWfmAutoInvisibleMins]  = useState(30);
  const [overlayStatus, setOverlayStatus] = useState("");
  // Without OCR the overlay can never trigger, so treat it as off for this run.
  // The stored preference is deliberately left untouched: the same settings file
  // is used on Windows, where the overlay does work.
  const [subsummedWarframes, setSubsummedWarframes] = useState<Set<string>>(new Set());
  const [archonShards, setArchonShards] = useState<Record<string, ArchonShard[]>>({});
  const [formaData, setFormaData] = useState<Record<string, number>>({});
  const wfmInvisibleOnStartRef  = useRef(false);
  const wfmInvisibleOnCloseRef  = useRef(false);
  const wfmLoggedInRef          = useRef(false);
  const catalogRef = useRef<CatalogItem[]>([]);
  const [changeLog, setChangeLog] = useState<ChangeLogEntry[]>([]);
  const [changeLogArrivalToken, setChangeLogArrivalToken] = useState(0);
  const [lastInventoryScanAt, setLastInventoryScanAt] = useState<number | null>(null);
  const [inventoryReady, setInventoryReady] = useState(false);
  const inventoryReadyRef = useRef(false);
  const [inventoryFilters, setInventoryFilters] = useState<InventoryFilters>(INVENTORY_FILTERS_DEFAULT);
  const { category, search, filterOwned, filterRecent, filterPrime, filterVaulted, filterUnvaulted, filterRank, sortMode } = inventoryFilters;
  const prevSortRef = useRef(sortMode);
  useEffect(() => { if (sortMode !== "recent") prevSortRef.current = sortMode; }, [sortMode]);
  const toggleInventoryRecent = useCallback(() => setInventoryFilters(previous => {
    const filterRecent = !previous.filterRecent;
    return { ...previous, filterRecent, sortMode: filterRecent ? "recent" : prevSortRef.current };
  }), []);
  const [inventoryView, setInventoryView] = useState<ViewMode>(() =>
    (localStorage.getItem(PREFERENCE_KEYS.INVENTORY_VIEW) as ViewMode | null) ?? "cards"
  );
  const setInventoryViewPreference = useCallback((view: ViewMode) => {
    setInventoryView(view);
    localStorage.setItem(PREFERENCE_KEYS.INVENTORY_VIEW, view);
  }, []);

  // ── Per-tab persisted filter state ────────────────────────────────────────
  const [lastChanged, setLastChanged] = useState<Record<string, number>>({});
  const [monitoring, setMonitoring] = useState(false);
  const [warframeRunning, setWarframeRunning] = useState(false);
  const [itemCount, setItemCount] = useState(0);
  const [recipeCount, setRecipeCount] = useState(0);
  const [fetching, setFetching] = useState(false);
  const [fetchMsg, setFetchMsg] = useState("");
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<'general' | 'overlays' | 'market' | 'accessibility' | 'data' | 'debugging'>('general');
  const [foundryPageSize, setFoundryPageSize] = useState<FoundryPageSize>(DEFAULT_FOUNDRY_PAGE_SIZE);
  const [overlayEnabledSetting, setOverlayEnabled] = useState<boolean>(
    () => localStorage.getItem(PREFERENCE_KEYS.OVERLAY_ENABLED) !== "false"
  );
  const overlayEnabled = overlayEnabledSetting;
  const [overlayPriority, setOverlayPriority] = useState<RelicOverlayPriority>(
    () => (localStorage.getItem(PREFERENCE_KEYS.OVERLAY_PRIORITY) ?? DEFAULT_RELIC_OVERLAY_PRIORITY) as RelicOverlayPriority
  );
  const [relicPickEnabled,    setRelicPickEnabled]    = useState<boolean>(true);
  const [memTriggerEnabled,   setMemTriggerEnabled]   = useState<boolean>(false);
  const [arbOverlayEnabled,   setArbOverlayEnabled]   = useState<boolean>(false);
  const [relicPickPriority,   setRelicPickPriority]   = useState<RelicPickPriority>(DEFAULT_RELIC_PICK_PRIORITY);
  const [relicPickRefinement, setRelicPickRefinement] = useState<RelicRefinement>(DEFAULT_RELIC_PICK_REFINEMENT);
  const [relicPickLines,      setRelicPickLines]      = useState<RelicPickLines>(DEFAULT_RELIC_PICK_LINES);
  const [appVersion, setAppVersion] = useState("");
  const [updateAvailable, setUpdateAvailable] = useState<UpdateAvailable | null>(null);
  const [showUpdateDialog, setShowUpdateDialog] = useState(false);
  const [showInventoryBatchPreview, setShowInventoryBatchPreview] = useState(false);
  // "scanning" while blob capture is running, "done" briefly after it finishes
  const [blobStage, setBlobStage] = useState<"scanning" | "done" | null>(null);
  const blobDoneTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [textScale, setTextScale] = useState(() => {
    const s = parseFloat(localStorage.getItem(PREFERENCE_KEYS.TEXT_SCALE) ?? "1");
    document.documentElement.style.setProperty("--ff-scale", s.toString());
    return s;
  });
  const [colorblindMode, setColorblindMode] = useState(() =>
    localStorage.getItem(PREFERENCE_KEYS.COLORBLIND_MODE) === "true"
  );
  const [clockFormat, setClockFormat] = useState<ClockFormat>(DEFAULT_CLOCK_FORMAT);
  const [itemsRefreshKey, setItemsRefreshKey] = useState(0);
  const [imgCacheDir, setImgCacheDir] = useState("");

  // ── Modular Window state ───────────────────────────────────────────────────
  const [tracked, setTracked] = useState<string[]>([]);
  const [favorites, setFavorites] = useState<string[]>([]);
  const [timerFavorites, setTimerFavorites] = useState<string[]>([]);
  const [fissureWatches, setFissureWatches] = useState<FissureWatch[]>([]);
  const [fissureNotifications, setFissureNotifications] = useState(true);
  const [arbFavorites, setArbFavorites] = useState<string[]>([]);
  const [arbLeadMins, setArbLeadMins] = useState(DEFAULT_LEAD_MINS);
  // The filter starts wide and the alert rule starts empty: showing every hour
  // is what a browser is for, while alerting is opt-in.
  const [arbTierFilter, setArbTierFilter] = useState<TierKey[]>([...TIER_KEYS]);
  const [arbAlertTiers, setArbAlertTiers] = useState<TierKey[]>([]);
  const [arbScheduleDays, setArbScheduleDays] = useState(DEFAULT_SCHEDULE_DAYS);
  const [modularWidth, setModularWidth] = useState(240);
  const [modularSectionOrder, setModularSectionOrder] = useState<string[]>([...MODULAR_SECTION_ORDER_DEFAULT]);
  const [modularPopout, setModularPopout] = useState(false);
  const modularWinRef = useRef<WebviewWindow | null>(null);
  const modularWinGeomRef = useRef<{ x?: number; y?: number; w?: number; h?: number }>({});

  const handleWfmLoginChange = useCallback((loggedIn: boolean) => {
    setWfmLoggedIn(loggedIn);
    wfmLoggedInRef.current = loggedIn;
  }, []);

  // ── Settings helpers ──────────────────────────────────────────────────────
  // Refs so we can read the latest state in the save callback without stale closures
  const settingsLoadedRef = useRef(false);
  const settingsRef = useRef<SettingsSnapshot>({
    overlayEnabled: true, overlayPriority: DEFAULT_RELIC_OVERLAY_PRIORITY, textScale: 1, colorblindMode: false, clockFormat: DEFAULT_CLOCK_FORMAT, memoryScannerEnabled: false, blobLogEnabled: false, autoDiagEnabled: false,
    tracked: [] as string[], favorites: [] as string[], timerFavorites: [] as string[], fissureWatches: [] as FissureWatch[], fissureNotifications: true, modularWidth: 240,
    arbitrationFavorites: [] as string[], arbitrationLeadMins: DEFAULT_LEAD_MINS, arbitrationOverlayEnabled: false,
    arbitrationTierFilter: [...TIER_KEYS] as TierKey[], arbitrationAlertTiers: [] as TierKey[],
    arbitrationScheduleDays: DEFAULT_SCHEDULE_DAYS,
    modularSectionOrder: ["tracking", "favorites", "timers"] as string[], modularPopout: false,
    wfmInvisibleOnStart: false, wfmInvisibleOnClose: false, wfmAutoInvisible: false, wfmAutoInvisibleMins: 30,
    relicPickEnabled: true, relicPickPriority: DEFAULT_RELIC_PICK_PRIORITY, relicPickRefinement: DEFAULT_RELIC_PICK_REFINEMENT, relicPickLines: DEFAULT_RELIC_PICK_LINES,
    foundryPageSize: DEFAULT_FOUNDRY_PAGE_SIZE,
    memTriggerEnabled: false,
  });
  settingsRef.current = { overlayEnabled: overlayEnabledSetting, overlayPriority, textScale, colorblindMode, clockFormat, memoryScannerEnabled, blobLogEnabled, autoDiagEnabled, tracked, favorites, timerFavorites, fissureWatches, fissureNotifications, arbitrationFavorites: arbFavorites, arbitrationLeadMins: arbLeadMins, arbitrationOverlayEnabled: arbOverlayEnabled, arbitrationTierFilter: arbTierFilter, arbitrationAlertTiers: arbAlertTiers, arbitrationScheduleDays: arbScheduleDays, modularWidth, modularSectionOrder, modularPopout, wfmInvisibleOnStart, wfmInvisibleOnClose, wfmAutoInvisible, wfmAutoInvisibleMins, relicPickEnabled, relicPickPriority, relicPickRefinement, relicPickLines, foundryPageSize, memTriggerEnabled };

  const saveAllSettings = useCallback(() => {
    // Until the on-disk settings have been applied, settingsRef still holds
    // the defaults (tracked/favorites empty). Saving the full object at that
    // point would overwrite the user's file with those defaults, so refuse.
    if (!settingsLoadedRef.current) {
      console.error("save_settings skipped: settings not loaded yet, saving now would clobber the file");
      return;
    }
    const settings: SettingsPatch = { ...settingsRef.current };
    invoke(TAURI_COMMANDS.SAVE_SETTINGS, { json: JSON.stringify(settings) }).catch((e) => {
      console.error("save_settings failed:", e);
    });
  }, []); // eslint-disable-line

  // ── Memory scanner toggle ─────────────────────────────────────────────────
  useEffect(() => {
    if (memoryScannerEnabled) {
      invoke("start_monitor").then(() => setMonitoring(true)).catch(() => {});
    } else {
      invoke("stop_monitor").then(() => setMonitoring(false)).catch(() => {});
    }
  }, [memoryScannerEnabled]); // eslint-disable-line

  // ── Blob log toggle ───────────────────────────────────────────────────────
  useEffect(() => {
    invoke("set_blob_log", { enabled: blobLogEnabled }).catch(() => {});
  }, [blobLogEnabled]); // eslint-disable-line

  // ── Debug data sizes — reload when the Debugging settings tab opens ─────────
  // ── Log watcher — always start regardless of memory scanner toggle ─────────
  // EE.log is plain file I/O (not memory reading) — handles riven detection,
  // trade completion, and WFM whisper detection unconditionally.
  useEffect(() => {
    invoke("start_log_watcher").catch(() => {});
  }, []); // eslint-disable-line

  // ── WFM auto-login at app start ───────────────────────────────────────────
  // Restores the session into Rust's AppState so the Trading tab is instantly
  // ready when the user opens it — no need to visit the tab first.
  // Also pre-warms the top-items cache in the background so the Statistics tab
  // loads instantly rather than running ~2 minutes of API calls on first open.
  useEffect(() => {
    (async () => {
      const creds = await invoke<WfmCredentials | null>(TAURI_COMMANDS.WFM_LOAD_CREDENTIALS).catch(() => null);
      if (creds) {
        const session = await invoke<WfmSession | null>(TAURI_COMMANDS.WFM_SET_JWT, { jwt: creds[1] }).catch(() => null);
        if (session) {
          setWfmLoggedIn(true);
          wfmLoggedInRef.current = true;
          if (wfmInvisibleOnStartRef.current) {
            invoke(TAURI_COMMANDS.WFM_SET_STATUS, { status: "invisible" }).catch(() => {});
          }
        }
      }
    })();
    // Fire-and-forget: populates WFM_TOP_CACHE so the Statistics tab is instant
    invoke(TAURI_COMMANDS.GET_WFM_TOP_ITEMS).catch(() => {});
    invoke<string>("get_img_cache_dir").then(setImgCacheDir).catch(() => {});
    invoke("prewarm_image_cache").catch(() => {});
  }, []); // eslint-disable-line

  // ── WFM: intercept window close to go invisible first ─────────────────────
  // Only runs in the main window — overlay/test windows must not call force_quit.
  useEffect(() => {
    if (_winLabel !== "main") return;
    let unlistenFn: (() => void) | null = null;
    getCurrentWindow().onCloseRequested(async event => {
      event.preventDefault();
      if (wfmInvisibleOnCloseRef.current && wfmLoggedInRef.current) {
        await Promise.race([
          invoke(TAURI_COMMANDS.WFM_SET_STATUS, { status: "invisible" }).catch(() => {}),
          new Promise<void>(resolve => setTimeout(resolve, 8000)),
        ]);
      }
      invoke("force_quit").catch(() => {});
    }).then(fn => { unlistenFn = fn; });
    return () => { unlistenFn?.(); };
  }, []); // eslint-disable-line

  // ── WFM: auto-invisible countdown timer ───────────────────────────────────
  useEffect(() => {
    if (!wfmAutoInvisible || !wfmLoggedIn) return;
    const id = setTimeout(() => {
      invoke(TAURI_COMMANDS.WFM_SET_STATUS, { status: "invisible" }).catch(() => {});
    }, wfmAutoInvisibleMins * 60 * 1000);
    return () => clearTimeout(id);
  }, [wfmAutoInvisible, wfmAutoInvisibleMins, wfmLoggedIn]);

  // ── Bootstrap ──────────────────────────────────────────────────────────────

  useEffect(() => {
    invoke<string[]>("get_saved_consumed_suits")
      .then(suits => { if (suits.length > 0) setSubsummedWarframes(new Set(suits)); })
      .catch(() => {});

    // Load user settings from file — survives reinstalls unlike localStorage
    invoke<string>(TAURI_COMMANDS.LOAD_SETTINGS).then(json => {
      // A missing file is a first launch: nothing to clobber, saving is safe.
      if (!json) { settingsLoadedRef.current = true; return; }
      try {
        const s = JSON.parse(json) as SettingsFile;
        if (typeof s.memoryScannerEnabled === "boolean") setMemoryScannerEnabled(s.memoryScannerEnabled);
        if (typeof s.blobLogEnabled === "boolean") setBlobLogEnabled(s.blobLogEnabled);
if (typeof s.autoDiagEnabled === "boolean") {
          setAutoDiagEnabled(s.autoDiagEnabled);
          localStorage.setItem(PREFERENCE_KEYS.AUTO_DIAGNOSTICS, String(s.autoDiagEnabled));
        }
        if (typeof s.overlayEnabled === "boolean") {
          setOverlayEnabled(s.overlayEnabled);
          localStorage.setItem(PREFERENCE_KEYS.OVERLAY_ENABLED, String(s.overlayEnabled));
        }
        if (typeof s.overlayPriority === "string") {
          setOverlayPriority(s.overlayPriority as RelicOverlayPriority);
          localStorage.setItem(PREFERENCE_KEYS.OVERLAY_PRIORITY, s.overlayPriority);
        }
        if (typeof s.textScale === "number") {
          setTextScale(s.textScale);
          document.documentElement.style.setProperty("--ff-scale", s.textScale.toString());
          localStorage.setItem(PREFERENCE_KEYS.TEXT_SCALE, s.textScale.toString());
        }
        if (typeof s.colorblindMode === "boolean") {
          setColorblindMode(s.colorblindMode);
          localStorage.setItem(PREFERENCE_KEYS.COLORBLIND_MODE, String(s.colorblindMode));
        }
        if (typeof s.clockFormat === "string" && CLOCK_FORMAT_OPTIONS.includes(s.clockFormat)) {
          setClockFormat(s.clockFormat as ClockFormat);
        }
        if (Array.isArray(s.tracked)) setTracked(s.tracked);
        if (Array.isArray(s.favorites)) setFavorites(s.favorites);
        if (Array.isArray(s.timerFavorites)) setTimerFavorites(s.timerFavorites);
        if (Array.isArray(s.fissureWatches)) {
          setFissureWatches(s.fissureWatches);
          restoredWatchIdsRef.current = new Set((s.fissureWatches as FissureWatch[]).map(w => w.id));
        }
        if (typeof s.fissureNotifications === "boolean") setFissureNotifications(s.fissureNotifications);
        if (Array.isArray(s.arbitrationFavorites)) setArbFavorites(s.arbitrationFavorites.filter((x: unknown) => typeof x === "string"));
        if (typeof s.arbitrationLeadMins === "number") setArbLeadMins(clampLead(s.arbitrationLeadMins));
        const storedFilter = sanitizeTierKeys(s.arbitrationTierFilter);
        if (storedFilter) setArbTierFilter(storedFilter);
        const storedAlertTiers = sanitizeTierKeys(s.arbitrationAlertTiers);
        if (storedAlertTiers) setArbAlertTiers(storedAlertTiers);
        if (typeof s.arbitrationScheduleDays === "number") setArbScheduleDays(clampScheduleDays(s.arbitrationScheduleDays));
        if (typeof s.arbitrationOverlayEnabled === "boolean") { setArbOverlayEnabled(s.arbitrationOverlayEnabled); invoke("set_arbitration_overlay_enabled", { enabled: s.arbitrationOverlayEnabled }); }
        if (Array.isArray(s.arbitrationAlertsFired)) arbFiredRef.current = s.arbitrationAlertsFired.filter((x: unknown) => typeof x === "string");
        if (typeof s.modularWidth === "number") setModularWidth(s.modularWidth);
        if (Array.isArray(s.modularSectionOrder)) {
          const order: string[] = s.modularSectionOrder;
          if (!order.includes("timers"))   order.push("timers");
          if (!order.includes("fissures")) order.push("fissures");
          setModularSectionOrder(order);
        }
        if (typeof s.modularPopout === "boolean") setModularPopout(s.modularPopout);
        if (typeof s.modularWinX === "number") modularWinGeomRef.current.x = s.modularWinX;
        if (typeof s.modularWinY === "number") modularWinGeomRef.current.y = s.modularWinY;
        if (typeof s.modularWinWidth === "number") modularWinGeomRef.current.w = s.modularWinWidth;
        if (typeof s.modularWinHeight === "number") modularWinGeomRef.current.h = s.modularWinHeight;
        if (typeof s.wfmInvisibleOnStart === "boolean") { setWfmInvisibleOnStart(s.wfmInvisibleOnStart); wfmInvisibleOnStartRef.current = s.wfmInvisibleOnStart; }
        if (typeof s.wfmInvisibleOnClose === "boolean") { setWfmInvisibleOnClose(s.wfmInvisibleOnClose); wfmInvisibleOnCloseRef.current = s.wfmInvisibleOnClose; }
        if (typeof s.wfmAutoInvisible    === "boolean") setWfmAutoInvisible(s.wfmAutoInvisible);
        if (typeof s.wfmAutoInvisibleMins === "number") setWfmAutoInvisibleMins(s.wfmAutoInvisibleMins);
        if (typeof s.relicPickEnabled    === "boolean") { setRelicPickEnabled(s.relicPickEnabled); invoke(TAURI_COMMANDS.SET_RELIC_PICK_ENABLED, { enabled: s.relicPickEnabled }); }
        if (typeof s.memTriggerEnabled   === "boolean") { setMemTriggerEnabled(s.memTriggerEnabled); invoke(TAURI_COMMANDS.SET_MEM_TRIGGER_ENABLED, { enabled: s.memTriggerEnabled }); }
        if (RELIC_PICK_PRIORITY_OPTIONS.includes(s.relicPickPriority)) setRelicPickPriority(s.relicPickPriority);
        if (RELIC_PICK_REFINEMENT_OPTIONS.includes(s.relicPickRefinement)) setRelicPickRefinement(s.relicPickRefinement);
        if (RELIC_PICK_LINES_OPTIONS.includes(s.relicPickLines)) setRelicPickLines(s.relicPickLines);
        if (FOUNDRY_PAGE_SIZE_OPTIONS.includes(s.foundryPageSize)) setFoundryPageSize(s.foundryPageSize);
      } catch {}
      // Unblock saving even if the file failed to parse, since the backend
      // refuses to overwrite a settings.json that is not a valid JSON object.
      settingsLoadedRef.current = true;
    }).catch(() => {});

    invoke<string | null>("get_player_name").then(name => { if (name) setPlayerName(name); }).catch(() => {});
    invoke<CatalogItem[]>(TAURI_COMMANDS.GET_ALL_ITEMS).then(items => { setCatalog(items); catalogRef.current = items; });
    invoke<QuantityMap>(TAURI_COMMANDS.GET_CURRENT_QUANTITIES)
      .then(setQuantities)
      .catch(() => {})
      .finally(() => {
        if (!inventoryReadyRef.current) {
          inventoryReadyRef.current = true;
          setInventoryReady(true);
        }
      });
    invoke<ChangeLogEntry[]>("get_change_log", { limit: 200 }).then(log => {
      setChangeLog(log);
      const lc: Record<string, number> = {};
      for (const c of log) lc[c.unique_name] = Math.max(lc[c.unique_name] ?? 0, c.timestamp);
      setLastChanged(lc);
    });
    invoke<ItemListStatus>("get_item_list_status").then(s => {
      setItemCount(s.count);
      setRecipeCount(s.recipe_count);
    });

    getVersion().then(setAppVersion).catch(() => {});

    // Auto-start monitor on launch — only if memory scanner is explicitly enabled
    invoke<boolean>("get_monitor_status").then(active => {
      if (!active) {
        // memoryScannerEnabled not yet loaded from settings at this point;
        // the effect below handles delayed auto-start after settings load.
      } else {
        setMonitoring(true);
      }
    });
  }, []);

  // Refresh diagnostics folder size every minute so the Clear button stays current.
  // ── Inventory update events ────────────────────────────────────────────────

  useEffect(() => {
    const unlisten = listen<InventoryUpdate>(TAURI_EVENTS.INVENTORY_UPDATE, (e) => {
      const p = e.payload;
      setLastInventoryScanAt(p.scanned_at);
      if (!inventoryReadyRef.current) {
        inventoryReadyRef.current = true;
        setInventoryReady(true);
      }
      // Only replace quantities if the content actually changed.
      // The monitor loop re-emits cached state periodically; without this guard
      // every emit triggers a full 17k-item useMemo rebuild cascade.
      setQuantities(prev => {
        const next = p.quantities;
        const prevKeys = Object.keys(prev);
        const nextKeys = Object.keys(next);
        if (prevKeys.length !== nextKeys.length) return next;
        for (const k of nextKeys) { if (next[k] !== prev[k]) return next; }
        return prev;
      });
      if (p.crafting) setCrafting(p.crafting);
      if (p.mastery_rank != null) setMasteryRank(p.mastery_rank);
      if (p.player_name) setPlayerName(p.player_name);
      if (p.mastery_data && (p.is_full_pass || Object.keys(p.mastery_data).length > 0))
        setMasteryData(p.mastery_data);
      if (p.owned_levels && (p.is_full_pass || Object.keys(p.owned_levels).length > 0))
        setOwnedLevels(p.owned_levels);
      setWarframeRunning(p.warframe_running);
      if (p.consumed_suits && p.consumed_suits.length > 0) {
        setSubsummedWarframes(prev => {
          const next = new Set(prev);
          for (const s of p.consumed_suits!) next.add(s);
          return next;
        });
      }
      if (p.mods && Object.keys(p.mods).length > 0) {
        setScannerMods(p.mods);
      }
      if (p.socketed_shards) {
        // In-memory color values use ACC_RED/BLUE/YELLOW/GREEN/PURPLE.
        // Tauforged variants include "TAU" in the string (e.g. ACC_TAU_RED).
        const SHARD_COLORS: { prefix: string; type: string; colorHex: string; tauHex: string }[] = [
          { prefix: "ACC_RED",    type: "Crimson",  colorHex: "#e04040", tauHex: "#ff7070" },
          { prefix: "ACC_BLUE",   type: "Azure",    colorHex: "#4488ff", tauHex: "#77aaff" },
          { prefix: "ACC_GREEN",  type: "Viridian", colorHex: "#44cc66", tauHex: "#66ff99" },
          { prefix: "ACC_YELLOW", type: "Amber",    colorHex: "#ffaa00", tauHex: "#ffcc44" },
          { prefix: "ACC_PURPLE", type: "Violet",   colorHex: "#9944ff", tauHex: "#bb77ff" },
        ];
        const INT_TO_ACC = ["ACC_RED","ACC_BLUE","ACC_GREEN","ACC_YELLOW","ACC_PURPLE"];
        const parsed: Record<string, { type: string; tauforged: boolean; color: string; boost?: string }[]> = {};
        for (const [wfPath, shards] of Object.entries(p.socketed_shards)) {
          parsed[wfPath] = shards.map(s => {
            let raw = s.color.toUpperCase();
            // If it's a pure integer, normalise to ACC_ string
            if (/^\d+$/.test(raw)) {
              const n = parseInt(raw);
              raw = INT_TO_ACC[n % 5] ?? raw;  // %5 so tau-forged (5-9) maps to base color
            }
            // In memory: tauforged shards use the suffix "_MYTHIC" (e.g. "ACC_RED_MYTHIC").
            const tauforged = raw.includes("MYTHIC") || raw.includes("TAU") || parseInt(s.color) >= 5;
            const entry = SHARD_COLORS.find(e => raw.startsWith(e.prefix));
            const colorInfo = entry ?? { type: "Unknown", colorHex: "#b0b0b0", tauHex: "#d0d0d0" };
            const seg = s.upgrade_type.split("/").pop() ?? "";
            const boostRaw = seg.replace(/^ArchonCrystalUpgrade(?:Warframe|Companion)?/, "");
            const boost = boostRaw.replace(/([A-Z])/g, " $1").trim() || undefined;
            // Tauforged shards use a brighter colour so they stand out from normal shards.
            const color = tauforged ? colorInfo.tauHex : colorInfo.colorHex;
            return { type: colorInfo.type, tauforged, color, boost };
          });
        }
        if (p.is_full_pass) {
          // Full pass = authoritative complete state; replace so removed shards don't linger.
          setArchonShards(parsed);
        } else if (Object.keys(parsed).length > 0) {
          setArchonShards(prev => ({ ...prev, ...parsed }));
        }
      }
      if (p.forma_counts) {
        if (p.is_full_pass) {
          setFormaData(p.forma_counts);
        } else if (Object.keys(p.forma_counts).length > 0) {
          setFormaData(prev => ({ ...prev, ...p.forma_counts }));
        }
      }
      if (p.changes.length > 0) {
        setChangeLog(prev => [...p.changes, ...prev].slice(0, 200));
        setChangeLogArrivalToken(token => token + 1);
        setLastChanged(prev => {
          const next = { ...prev };
          for (const c of p.changes) next[c.unique_name] = c.timestamp;
          return next;
        });
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // ── Blob processing status ────────────────────────────────────────────────
  useEffect(() => {
    const unlisten = listen<BlobStatusPayload>("blob-status", e => {
      const { stage } = e.payload;
      if (stage === "scanning") {
        if (blobDoneTimerRef.current) clearTimeout(blobDoneTimerRef.current);
        setBlobStage("scanning");
      } else if (stage === "done") {
        setBlobStage("done");
        blobDoneTimerRef.current = setTimeout(() => setBlobStage(null), 4000);
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // The launch check fires at most once, so opening the dialog here cannot nag:
  // dismissing it leaves only the header badge until the next launch or a
  // manual check.
  useEffect(() => {
    const show = (u: UpdateAvailable) => {
      setUpdateAvailable(u);
      setShowUpdateDialog(true);
    };
    const unlisten = onUpdateAvailable(show);
    pendingUpdate().then(u => { if (u) show(u); }).catch(() => {});
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // ── Player name (immediate, from EE.log "Logged in NAME") ───────────────
  useEffect(() => {
    const unlisten = listen<string>("player-name", e => {
      setPlayerName(e.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // ── Sync modular state from pop-out window ────────────────────────────────
  // When the pop-out saves (unstar, reorder), Rust emits settings-updated.
  // Compare before setting to avoid a save → emit → re-read → save loop.
  useEffect(() => {
    const unlisten = listen(TAURI_EVENTS.SETTINGS_UPDATED, () => {
      invoke<string>(TAURI_COMMANDS.LOAD_SETTINGS).then(json => {
        if (!json) return;
        try {
          const s = JSON.parse(json) as SettingsFile;
          const cur = settingsRef.current;
          if (Array.isArray(s.favorites) && JSON.stringify(s.favorites) !== JSON.stringify(cur.favorites))
            setFavorites(s.favorites);
          if (Array.isArray(s.tracked) && JSON.stringify(s.tracked) !== JSON.stringify(cur.tracked))
            setTracked(s.tracked);
          if (Array.isArray(s.modularSectionOrder) && JSON.stringify(s.modularSectionOrder) !== JSON.stringify(cur.modularSectionOrder))
            setModularSectionOrder(s.modularSectionOrder);
        } catch {}
      }).catch(() => {});
      // The app creates the riven window once and caches it, so a scale change
      // must resize it in place. Its height follows the game window, not the scale.
      resizeRivenForScale();
    });
    return () => { unlisten.then(fn => fn()); };
  }, []); // eslint-disable-line

  // ── Persist main window geometry on move/resize ───────────────────────────
  useEffect(() => {
    const win = getCurrentWindow();
    let t: ReturnType<typeof setTimeout> | null = null;
    const save = () => {
      if (t) clearTimeout(t);
      t = setTimeout(() => {
        Promise.all([win.outerPosition(), win.outerSize()]).then(([pos, size]) => {
          const patch: SettingsPatch = {
            windowX: pos.x, windowY: pos.y,
            windowWidth: size.width, windowHeight: size.height,
          };
          invoke(TAURI_COMMANDS.SAVE_SETTINGS, { json: JSON.stringify(patch) }).catch(() => {});
        }).catch(() => {});
      }, 400);
    };
    const unlistenMove = win.onMoved(save);
    const unlistenResize = win.onResized(save);
    return () => {
      if (t) clearTimeout(t);
      unlistenMove.then(fn => fn());
      unlistenResize.then(fn => fn());
    };
  }, []); // eslint-disable-line

  const toggleTracked = useCallback((id: string) => {
    setTracked(prev => prev.includes(id) ? prev.filter(i => i !== id) : [...prev, id]);
  }, []);

  const toggleFavorite = useCallback((id: string) => {
    setFavorites(prev => prev.includes(id) ? prev.filter(i => i !== id) : [...prev, id]);
  }, []);

  // ── Fetch item list ────────────────────────────────────────────────────────

  const handleFetch = async () => {
    setFetching(true);
    setFetchMsg("Fetching…");
    // Stop monitor during refresh so it restarts with the new item list
    const wasMonitoring = monitoring;
    if (wasMonitoring) {
      await invoke("stop_monitor");
      setMonitoring(false);
    }
    try {
      const count = await invoke<number>("fetch_item_list", { force: true });
      setItemCount(count);
      const items = await invoke<CatalogItem[]>(TAURI_COMMANDS.GET_ALL_ITEMS);
      setCatalog(items);
      catalogRef.current = items;
      const status = await invoke<ItemListStatus>("get_item_list_status");
      setRecipeCount(status.recipe_count);
      setFetchMsg(`Loaded ${count.toLocaleString()} items, ${status.recipe_count.toLocaleString()} recipes`);
      setItemsRefreshKey(k => k + 1);
      invoke("prewarm_image_cache").catch(() => {});
    } catch (e) {
      setFetchMsg(`Error: ${e}`);
    } finally {
      setFetching(false);
      if (wasMonitoring) {
        await invoke("start_monitor");
        setMonitoring(true);
      }
    }
  };

  // The catalogue the backend loaded from disk is already on screen; revalidate
  // it behind the UI so a launch never waits on the network, and leave the
  // monitor running since the refresh is a no-op while the cache is fresh.
  useEffect(() => {
    invoke<number>("fetch_item_list").then(async count => {
      setItemCount(count);
      const items = await invoke<CatalogItem[]>("get_all_items");
      setCatalog(items);
      catalogRef.current = items;
      const status = await invoke<{ count: number; recipe_count: number }>("get_item_list_status");
      setRecipeCount(status.recipe_count);
      setItemsRefreshKey(k => k + 1);
      invoke("prewarm_image_cache").catch(() => {});
    }).catch(e => setFetchMsg(`Error: ${e}`));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (settingsLoadedRef.current) saveAllSettings();
  }, [tracked, favorites, timerFavorites, fissureWatches, fissureNotifications, arbFavorites, arbLeadMins, arbTierFilter, arbAlertTiers, arbScheduleDays, modularWidth, memoryScannerEnabled, blobLogEnabled, autoDiagEnabled, modularSectionOrder, modularPopout]); // eslint-disable-line

  // ── Watched fissure notifications ──────────────────────────────────────────
  //
  // Lives here rather than in TimerHelper because TimerHelper only mounts while
  // its tab is open, and the whole point is to hear about a fissure while
  // looking at something else. The pop-out window deliberately does not run
  // this — two windows would otherwise notify twice for the same fissure.

  const { worldState } = useWorldState();

  const seenFissuresRef = useRef<SeenFissures>(new Map());
  // Watch IDs that should not fire on the first poll they're seen —
  // populated from saved settings on load, and extended whenever the user
  // adds a new watch mid-session (so adding a watch doesn't immediately
  // announce every currently-live fissure that matches it).
  const restoredWatchIdsRef = useRef<Set<string>>(new Set());
  const knownWatchIdsRef    = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (!worldState) return;

    // Any watch added since the last render is treated as "restored" so it
    // doesn't immediately alert for fissures that are already live.
    for (const w of fissureWatches) {
      if (!knownWatchIdsRef.current.has(w.id)) {
        restoredWatchIdsRef.current.add(w.id);
        knownWatchIdsRef.current.add(w.id);
      }
    }

    const { fresh, live } = collectNewMatches(worldState, fissureWatches, seenFissuresRef.current, restoredWatchIdsRef.current);
    // Tracked even while notifications are off, so switching them back on does
    // not announce everything that rotated in during the quiet period.
    seenFissuresRef.current = live;
    if (!fissureNotifications || fresh.length === 0) return;

    const suffix = (variant: string, sep: string) =>
      variant === "hard" ? `${sep}Steel Path` : variant === "storm" ? `${sep}Void Storm` : "";

    if (fresh.length === 1) {
      const { f, variant } = fresh[0];
      // fmtMs renders a dash once the clock runs out, which would read as
      // "Tessera (Void) — — left" for a fissure that expired mid-poll.
      const remaining = new Date(f.expiry).getTime() - Date.now();
      void notify(
        `${f.tier} ${f.missionType}${suffix(variant, " · ")}`,
        remaining > 0 ? `${f.node} — ${fmtMs(remaining)} left` : f.node,
      );
    } else {
      // A rotation can bring up a dozen matches at once, and a toast each is
      // enough to make anyone turn the feature off.
      const shown = fresh.slice(0, 5).map(({ f, variant }) =>
        `${f.tier} ${f.missionType}${suffix(variant, " ")} — ${f.node}`);
      if (fresh.length > shown.length) shown.push(`+${fresh.length - shown.length} more`);
      void notify(`${fresh.length} new fissures`, shown.join("\n"));
    }
  }, [worldState, fissureWatches, fissureNotifications]);

  // ── Arbitration alerts ─────────────────────────────────────────────────────
  //
  // Here rather than in Arbitrations, for the same reason the fissure alerts
  // are: that module is unmounted whenever another one is on screen.

  // Persisted, so a restart inside the lead window does not alert a second
  // time for the same hour.
  const arbFiredRef = useRef<string[]>([]);
  const arbAlertsOn = arbFavorites.length > 0 || arbAlertTiers.length > 0;

  const { schedule: arbSchedule, error: arbScheduleError } = useArbitrationSchedule(arbAlertsOn);

  // The loop reads its inputs from here rather than from the effect closure, so
  // starring a node changes what the next tick sees without tearing the timer
  // down and starting a fresh pass on top of one already running. This effect
  // has to stay above the loop's own, which reads the ref on its first tick.
  const arbInputsRef = useRef({ entries: [] as ScheduleEntry[], rule: {} as AlertRule, leadMins: DEFAULT_LEAD_MINS });
  useEffect(() => {
    arbInputsRef.current = {
      entries: arbSchedule?.entries ?? [],
      rule: { favorites: arbFavorites, tiers: arbAlertTiers },
      leadMins: arbLeadMins,
    };
  });

  // A pass outlives its tick whenever the notification IPC is slow, and two
  // passes reading the same fired state would raise one occurrence twice.
  const arbCheckingRef = useRef(false);

  // Held here so a denial survives Arbitrations' unmount, but written only by
  // that module: it raises the prompt on a gesture and shows the warning.
  const [arbPermissionDenied, setArbPermissionDenied] = useState(false);

  useEffect(() => {
    if (arbAlertsOn && arbScheduleError) {
      console.error("arbitration schedule unavailable, alerts paused:", arbScheduleError);
    }
  }, [arbAlertsOn, arbScheduleError]);

  useEffect(() => {
    const check = async () => {
      // A prune raises nothing, so it need not wait for a pass already running;
      // queuing it behind the guard would drop it, since unstarring the last
      // node also stops the timer that would otherwise come back to it.
      if (arbAlertsOn && arbCheckingRef.current) return;
      arbCheckingRef.current = true;
      try {
        const { entries, rule, leadMins } = arbInputsRef.current;
        const nowMs = Date.now();
        const fired = await runAlertPass(
          entries, rule, leadMins, arbFiredRef.current, nowMs / 1000,
          e => notify(
            `Arbitration — ${e.node}${e.region ? ` (${e.region})` : ""}`,
            `${[e.mission_type, e.faction].filter(Boolean).join(" · ")} — ${e.start * 1000 > nowMs
              ? `starts in ${fmtMs(e.start * 1000 - nowMs)}`
              : `under way, ${fmtMs(e.end * 1000 - nowMs)} left`}`,
          ));
        if (fired === null) return;
        arbFiredRef.current = fired;
        invoke("save_settings", { json: JSON.stringify({ arbitrationAlertsFired: fired }) })
          .catch(e => console.error("saving arbitration alert state failed", e));
      } finally {
        arbCheckingRef.current = false;
      }
    };

    // Unstarring the last node still leaves keys behind, so one pass runs to
    // prune them; only a user with favorites keeps the timer.
    void check();
    if (!arbAlertsOn) return;
    const poll = setInterval(check, EVAL_INTERVAL_MS);
    return () => clearInterval(poll);
  }, [arbAlertsOn]);

  // ── Modular pop-out window ─────────────────────────────────────────────────
  useEffect(() => {
    if (modularPopout) {
      if (modularWinRef.current) return;
      const g = modularWinGeomRef.current;

      // Only restore saved position if it lands on a currently connected monitor.
      // Guards against secondary monitor being unplugged since last session.
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

  // ── Riven overlay ─────────────────────────────────────────────────────────
  // Window state lives in module-level _rivenWin (above) — unaffected by StrictMode.
  // No pre-creation: show() silently fails on visible:false windows with this config.
  // Fresh window created on trigger (shows correctly); existing visible window reused for cycling.
  useEffect(() => {
    // Core OCR + overlay display. Called manually via button or (future) auto-detection.
    const runRivenCheck = async () => {
      _rivenLastTriggerMs = Date.now();
      _rivenRollCount++;
      const { emit } = await import("@tauri-apps/api/event");

      let rect: WarframeWindowRect = [0, 0, 0, 800];
      try { rect = await invoke<WarframeWindowRect>("get_warframe_window_rect"); } catch {}
      const [wx, wy, , wh] = rect;
      const result = await ensureRivenWindow(wx, wy, wh);
      let pendingPayload: RivenAnalysisUpdate | null = null;
      let windowReady = false;

      if (result && !result.fresh) {
        // Existing window — reset overlay state
        await emit(TAURI_EVENTS.RIVEN_SCANNING_START, {}).catch(() => {});
        windowReady = true;
      } else if (result?.fresh) {
        // Fresh window — send data once its listener signals ready
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
        const payload: RivenAnalysisUpdate = { analysis, ocrRaw: ocrResult.raw, weapon: ocrResult.weapon, positives: ocrResult.positives, negatives: ocrResult.negatives, rolledStats: ocrResult.rolled_stats, isComparison: ocrResult.is_comparison, originalStats: ocrResult.original_rolled_stats, rollCount: _rivenRollCount };
        if (windowReady) { await emit(TAURI_EVENTS.RIVEN_ANALYSIS_UPDATE, payload).catch(() => {}); }
        else              { pendingPayload = payload; }
      } catch (e) {
        await invoke("ocr_riven_log_error", { error: String(e) }).catch(() => {});
        const payload: RivenAnalysisUpdate = { analysis: null, ocrRaw: `OCR ERROR: ${e}`, weapon: "", positives: [], negatives: [], rolledStats: [], isComparison: false, originalStats: [], rollCount: _rivenRollCount };
        if (windowReady) { await emit(TAURI_EVENTS.RIVEN_ANALYSIS_UPDATE, payload).catch(() => {}); }
        else              { pendingPayload = payload; }
      }
    };

    // Wire module-level trigger so "Check Riven" button and "Start Comparison" can call it
    _rivenManualTrigger = () => { runRivenCheck().catch(() => {}); };

    // overlay "Start Comparison" button emits this event
    const unsubManual = listen(TAURI_EVENTS.RIVEN_MANUAL_CHECK, () => runRivenCheck().catch(() => {}));

    // Open trigger: EE.log watcher fires "riven-screen-open" via FindFirstChangeNotificationW
    // (instant file-write notification — no polling delay).
    // 4 s cooldown prevents double-fires from the same log buffer flush.
    const triggerOpen = () => {
      const now = Date.now();
      if (now - _rivenLastTriggerMs < 4000) return;
      runRivenCheck().catch(() => {});
    };
    const unsubAutoDetect = listen("riven-screen-open", () => triggerOpen());

    // Close triggers: EE.log (DiegeticArtifactCards HudVis 0) + manual dismiss.
    const unsubClose   = listen("riven-screen-close",   () => rivenWinHide("screen-close"));
    const unsubHideReq = listen<{ reason?: string }>(TAURI_EVENTS.RIVEN_OVERLAY_HIDE, e => rivenWinHide(e.payload?.reason ?? "overlay-hide"));

    return () => {
      unsubManual.then(fn => fn());
      unsubAutoDetect.then(fn => fn());
      unsubClose.then(fn => fn());
      unsubHideReq.then(fn => fn());
      _rivenManualTrigger = null;
    };
  }, []); // eslint-disable-line

  // ── Relic reward overlay ──────────────────────────────────────────────────
  // The overlay window is pre-declared in tauri.conf.json (y=-3000, off-screen).
  // We never create/destroy it — just reposition it on-screen and back off-screen.
  // This avoids a Windows deadlock: WebviewWindowBuilder::build() called dynamically
  // (from tokio threads, run_on_main_thread, or JS new WebviewWindow) all hang because
  // the Win32 event loop cannot process messages while our code is executing.
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
      // The strip's top edge aligns with the reward row in the game, so the scale
      // may only extend the strip downwards. Moving that edge would break the
      // alignment. The space below it is the limit, and past that the content is
      // clipped.
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
        // get_warframe_window_rect failed (Warframe may be in a different state).
        // Fall back to screen dimensions so the overlay still moves on-screen and
        // WebView2 un-freezes its JS before relic-rewards arrives.
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
      // Overlay.tsx already receives this event directly from Rust's global emit.
      // We only need to ensure the overlay window is on-screen; no forwarding needed
      // (forwarding via emitTo caused an infinite feedback loop in Tauri 2).
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
  }, []);

  // ── In-game trade detection ───────────────────────────────────────────────
  // Rust emits "trade-completed" when "The trade was successful!" is detected in
  // EE.log. One event covers ALL items from both sides of the trade session.
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
        // Gave items, received platinum — put plat on the first row only
        for (let i = 0; i < p.offeredItems.length; i++) {
          const item = p.offeredItems[i];
          await save("sold", item.name, item.qty, i === 0 ? p.receivedPlat : 0);
        }
      } else if (p.tradeType === "purchase") {
        // Gave platinum, received items — put plat on the first row only
        for (let i = 0; i < p.receivedItems.length; i++) {
          const item = p.receivedItems[i];
          await save("bought", item.name, item.qty, i === 0 ? p.offeredPlat : 0);
        }
      } else {
        // Item-for-item trade
        for (const item of p.offeredItems)  await save("traded-out", item.name, item.qty, 0);
        for (const item of p.receivedItems) await save("traded-in",  item.name, item.qty, 0);
      }
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  // ── Derived data ───────────────────────────────────────────────────────────

  // Central inventory: keyed by display name AND unique_name path (alias).
  // Both inventory["Ash Prime"] and inventory["/Lotus/Powersuits/Ninja/AshPrime"] resolve to the same entry.
  const inventory = useMemo(() => {
    const pathToCatalog = new Map<string, CatalogItem>();
    for (const item of catalog) pathToCatalog.set(item.unique_name, item);

    const allPaths = new Set([
      ...Object.keys(quantities),
      ...Object.keys(masteryData),
      ...Object.keys(archonShards),
      ...Object.keys(scannerMods),
      ...subsummedWarframes,
    ]);

    const inv: Record<string, InventoryItem> = {};
    for (const path of allPaths) {
      const cat = pathToCatalog.get(path);
      const name = cat?.name ?? path;
      let qty = quantities[path] ?? 0;
      if (scannerMods[path]) qty = Math.max(qty, scannerMods[path].total);
      if (subsummedWarframes.has(path)) qty = 0;

      const entry: InventoryItem = {
        unique_name:   path,
        quantity:      qty,
        mastery_rank:  masteryData[path] ?? 0,
        owned_levels:  ownedLevels[path] ?? [],
        archon_shards: archonShards[path] ?? [],
        forma_count:   formaData[path] ?? 0,
        subsumed:      subsummedWarframes.has(path),
        vaulted:       cat?.vaulted ?? null,
        category:      cat?.category ?? "",
        ducat_price:   cat?.ducats ?? null,
        wfm_price:     null,
        image_name:    cat?.image_name ?? null,
        mastery_req:   cat?.mastery_req ?? null,
        max_level_cap: cat?.max_level_cap ?? null,
      };
      inv[name] = entry;
      if (path !== name) inv[path] = entry; // path alias so existing unique_name lookups still work
    }
    return inv;
  }, [catalog, quantities, masteryData, ownedLevels, archonShards, formaData, subsummedWarframes, scannerMods]);

  const modCopiesMap = useMemo(() => {
    const map: Record<string, ModCopy[]> = {};
    for (const [path, mc] of Object.entries(scannerMods)) {
      map[path] = Object.entries(mc.by_rank)
        .map(([rankStr, count]) => ({ uniqueName: path, rank: parseInt(rankStr), count }))
        .sort((a, b) => b.rank - a.rank);
    }
    return map;
  }, [scannerMods]);

  const inventorySynced = Object.keys(quantities).length > 0;

  const availableRanks = useMemo(() => {
    const set = new Set<number>();
    for (const copies of Object.values(modCopiesMap))
      for (const c of copies) if (c.rank > 0) set.add(c.rank);
    return [...set].sort((a, b) => a - b);
  }, [modCopiesMap]);

  // Total counts only depend on the catalog — stable until item list is refreshed.
  const categoryTotals = useMemo(() => {
    const total: Record<string, number> = { all: catalog.length };
    for (const item of catalog) total[item.category] = (total[item.category] ?? 0) + 1;
    return total;
  }, [catalog]);

  // Owned counts depend on quantities — recalculates every inventory scan.
  const categoryOwned = useMemo(() => {
    const owned: Record<string, number> = { all: 0 };
    for (const item of catalog) {
      if ((inventory[item.unique_name]?.quantity ?? 0) > 0) {
        owned.all++;
        owned[item.category] = (owned[item.category] ?? 0) + 1;
      }
    }
    return owned;
  }, [catalog, inventory]);

  const categoryCounts = useMemo(
    () => ({ owned: categoryOwned, total: categoryTotals }),
    [categoryOwned, categoryTotals]
  );

  const favoritesSet = useMemo(() => new Set(favorites), [favorites]);

  const changeLogMap = useMemo(() => {
    const m = new Map<string, ChangeLogEntry[]>();
    for (const c of changeLog) {
      const arr = m.get(c.unique_name);
      if (arr) arr.push(c);
      else m.set(c.unique_name, [c]);
    }
    return m;
  }, [changeLog]);

  const craftingMap = useMemo(() => {
    const m = new Map<string, CraftingJob>();
    for (const c of crafting) m.set(c.unique_name, c);
    return m;
  }, [crafting]);

  const visibleItems = useMemo(() => {
    const q = search.toLowerCase();
    // Changelog order map: lower index = more recent position in changelog
    const changeOrder = new Map<string, number>();
    changeLog.forEach((c, i) => { if (!changeOrder.has(c.unique_name)) changeOrder.set(c.unique_name, i); });
    const out: (CatalogItem & { qty: number })[] = [];
    for (const i of catalog) {
      if (i.name === "Blueprint") continue;
      if (category !== "all" && i.category !== category) continue;
      if (q && !i.name.toLowerCase().includes(q)) continue;
      const qty = inventory[i.unique_name]?.quantity ?? 0;
      if (filterOwned    && qty === 0) continue;
      if (filterRecent   && lastChanged[i.unique_name] == null) continue;
      if (filterPrime    && !i.name.includes("Prime") && i.vaulted == null) continue;
      if (filterVaulted  && i.vaulted !== true) continue;
      if (filterUnvaulted && i.vaulted !== false) continue;
      if (filterRank !== null) {
        if (i.category === "Mods" || i.category === "Arcanes") {
          const copies = modCopiesMap[i.unique_name];
          if (!copies) continue;
          if (filterRank === "unranked") {
            if (!copies.some(c => c.rank === 0)) continue;
          } else {
            if (!copies.some(c => c.rank === filterRank)) continue;
          }
        }
      }
      out.push({ ...i, qty });
    }
    out.sort((a, b) => {
      if (sortMode === "recent" || filterRecent) {
        const at = lastChanged[a.unique_name] ?? 0;
        const bt = lastChanged[b.unique_name] ?? 0;
        if (bt !== at) return bt - at;
        // Tiebreak by changelog arrival order (lower index = more recent)
        const ai = changeOrder.get(a.unique_name) ?? Infinity;
        const bi = changeOrder.get(b.unique_name) ?? Infinity;
        return ai - bi || a.name.localeCompare(b.name);
      }
      const aOwned = a.qty > 0 ? 1 : 0;
      const bOwned = b.qty > 0 ? 1 : 0;
      if (bOwned !== aOwned) return bOwned - aOwned;
      if (sortMode === "name-asc")  return a.name.localeCompare(b.name);
      if (sortMode === "name-desc") return b.name.localeCompare(a.name);
      if (sortMode === "qty-asc")   return a.qty - b.qty || a.name.localeCompare(b.name);
      return b.qty - a.qty || a.name.localeCompare(b.name);
    });
    return out.slice(0, 1000);
  }, [catalog, inventory, inventoryFilters, lastChanged, modCopiesMap, changeLog]);

  const resetInventoryFilters = ({
    recent,
    searchTerm = "",
    categoryId = "all",
  }: {
    recent: boolean;
    searchTerm?: string;
    categoryId?: string;
  }) => {
    setActiveModule("inventory");
    setInventoryFilters(previous => ({
      ...INVENTORY_FILTERS_DEFAULT,
      category: categoryId,
      search: searchTerm,
      filterRecent: recent,
      sortMode: recent ? "recent" : previous.sortMode,
    }));
  };

  // Navigate to an item from the changelog — only switches module and sets search,
  // leaving any existing inventory filters in place (chip filters the user set should survive).
  const openChangeLogItem = (uniqueName: string) => {
    const item = catalog.find(candidate => candidate.unique_name === uniqueName);
    setActiveModule("inventory");
    setInventoryFilters(previous => ({ ...previous, search: item?.name ?? "" }));
  };

  const openRecentChanges = () => resetInventoryFilters({ recent: true });
  const openRecentCategory = (categoryId: string) => resetInventoryFilters({ recent: true, categoryId });
  const closeInventoryBatchPreview = useCallback(() => setShowInventoryBatchPreview(false), []);
  const handleInventoryContextMenu = useCallback((e: React.MouseEvent) => {
    const name = extractItemName(e);
    if (name) {
      e.preventDefault();
      openCtx(e.clientX, e.clientY, [
        { label: "Open Wiki", action: () => openWiki(name) },
        { label: "Copy Wiki Link", action: () => copyWikiLink(name) },
      ]);
    }
  }, [openCtx]);

  // ─── Render ─────────────────────────────────────────────────────────────────

  return (
    <ImgCacheDirContext.Provider value={imgCacheDir}>
    <div className="shell">

      {/* ── Header ── */}
      <header className="header">
        <span className="header-title">FrameForge</span>
        <HeaderStatusBadges
          masteryRank={masteryRank}
          playerName={playerName}
          updateVersion={updateAvailable?.version ?? null}
          inventoryLoaded={blobStage === "done"}
          onOpenUpdate={() => setShowUpdateDialog(true)}
        />
        <div className="header-right">
          {/* ── Connection status chips ── */}
          {(() => {
            // Memory chip
            const scanState: "online"|"warn"|"offline"|"disabled" =
              !memoryScannerEnabled ? "disabled"
              : warframeRunning     ? "online"
              : "offline";
            const scanDetail =
              !memoryScannerEnabled ? "OFF"
              : !monitoring         ? "Idle"
              : warframeRunning     ? "Scanning"
              : poking              ? "Checking…"
              : "No Game";

            // WFM chip
            const wfmState: "online"|"offline" = wfmLoggedIn ? "online" : "offline";
            const wfmDetail = wfmLoggedIn ? "Online" : "Not logged in";

            return (
              <>
                <ConnectionStatusChip
                  label="Memory"
                  state={scanState}
                  detail={scanDetail}
                  title={!memoryScannerEnabled ? "Memory scanner disabled — enable in Settings" : warframeRunning ? "Warframe detected — scanning memory" : "Click to recheck for Warframe"}
                  onClick={
                    !memoryScannerEnabled ? () => setShowSettings(true)
                    : !warframeRunning && monitoring ? () => {
                        setPoking(true);
                        invoke("poke_scan").finally(() => setTimeout(() => setPoking(false), 3000));
                      }
                    : undefined
                  }
                />
                <ConnectionStatusChip
                  label="WFM"
                  state={wfmState}
                  detail={wfmDetail}
                  title={wfmLoggedIn ? "Logged in to warframe.market" : "Not logged in to warframe.market — open the Market tab to log in"}
                  onClick={!wfmLoggedIn ? () => setActiveModule("market") : undefined}
                />
                {overlayStatus && (
                  <span className="conn-chip conn-overlay">
                    <span className="conn-dot" />
                    <span className="conn-detail">{overlayStatus}</span>
                  </span>
                )}
                <CacheStatusChip />
              </>
            );
          })()}
          <HeaderActions
            onOpenExternalUrl={url => invoke(TAURI_COMMANDS.OPEN_URL, { url }).catch(() => {})}
            onOpenSettings={() => {
              setShowSettings(true);
              getVersion().then(version => setAppVersion(version)).catch(() => {});
            }}
          />
        </div>
      </header>

      {showSettings && <SettingsModal onClose={() => setShowSettings(false)} {...{ settingsTab, setSettingsTab, foundryPageSize, setFoundryPageSize, settingsRef, saveAllSettings, memoryScannerEnabled, setMemoryScannerEnabled, modularPopout, setModularPopout, overlayStatus, overlayEnabled, setOverlayEnabled, overlayPriority, setOverlayPriority, memTriggerEnabled, setMemTriggerEnabled, relicPickEnabled, setRelicPickEnabled, relicPickPriority, setRelicPickPriority, relicPickLines, setRelicPickLines, wfmLoggedIn, wfmInvisibleOnStart, setWfmInvisibleOnStart, wfmInvisibleOnStartRef, wfmInvisibleOnClose, setWfmInvisibleOnClose, wfmInvisibleOnCloseRef, wfmAutoInvisible, setWfmAutoInvisible, wfmAutoInvisibleMins, setWfmAutoInvisibleMins, colorblindMode, setColorblindMode, textScale, setTextScale, clockFormat, setClockFormat, itemCount, recipeCount, handleFetch, fetching, fetchMsg, setQuantities, setScannerMods, setMasteryData, setArchonShards, setFormaData, setChangeLog, setLastChanged, setItemsRefreshKey, blobLogEnabled, setBlobLogEnabled, setShowInventoryBatchPreview, autoDiagEnabled, setAutoDiagEnabled, appVersion, arbOverlayEnabled, setArbOverlayEnabled }} onUpdateFound={u => { setUpdateAvailable(u); setShowUpdateDialog(true); }} />}
      {showUpdateDialog && updateAvailable && (
        <UpdateDialog update={updateAvailable} onDismiss={() => setShowUpdateDialog(false)} />
      )}

      {showInventoryBatchPreview && <InventoryBatchPreview onClose={closeInventoryBatchPreview} />}

      <div className="body">

        <AppNavigation activeModule={activeModule} onModuleChange={setActiveModule} />

        <div className="app-content">
        <div className="module-content">
        {/* ── Inventory module ── */}
        {activeModule === "inventory" && (
          <>
            <InventorySidebar
              categories={CATEGORIES}
              category={category}
              categoryCounts={categoryCounts}
              onCategoryChange={category => setInventoryFilters(previous => ({ ...previous, category }))}
              itemCount={itemCount}
              recipeCount={recipeCount}
              onFetch={handleFetch}
              fetching={fetching}
              fetchMsg={fetchMsg}
            />

            <div className="main">
              {monitoring && warframeRunning && !inventorySynced && (
                <div className="sync-banner">
                  Inventory not synced yet — complete a mission or visit a relay to load your inventory
                </div>
              )}

              <InventoryToolbar
                filters={inventoryFilters}
                onFiltersChange={setInventoryFilters}
                onToggleRecent={toggleInventoryRecent}
                availableRanks={availableRanks}
                showRankFilters={Object.keys(modCopiesMap).length > 0}
                itemCount={visibleItems.length}
                view={inventoryView}
                onViewChange={setInventoryViewPreference}
              />

              <InventoryGrid
                items={visibleItems}
                loading={!inventoryReady}
                monitoring={monitoring}
                view={inventoryView}
                inventory={inventory}
                modCopies={modCopiesMap}
                favorites={favoritesSet}
                lastChanged={lastChanged}
                changes={changeLogMap}
                crafting={craftingMap}
                filterRank={filterRank}
                onToggleFavorite={toggleFavorite}
                onContextMenu={handleInventoryContextMenu}
              />

            </div>
          </>
        )}

        {ctxMenu && <CtxMenu state={ctxMenu} onClose={closeCtx} />}

        {/* ── Foundry module ── */}
        {activeModule === "foundry" && (
          <ErrorBoundary>
            <Foundry inventory={inventory} refreshKey={itemsRefreshKey} crafting={crafting} colorblindMode={colorblindMode} subsummedWarframes={subsummedWarframes} tracked={tracked} onTrackToggle={toggleTracked} pageSize={foundryPageSize} />
          </ErrorBoundary>
        )}

        {/* ── Market Helper module ── */}
        {/* Keep mounted at all times so WfmTrading's trade-completed listener
            (auto listing update) fires regardless of which tab is active. */}
        <div style={{ display: activeModule === "market" ? "contents" : "none" }}>
          <MarketHelper inventory={inventory} refreshKey={itemsRefreshKey} crafting={crafting} onWfmLoginChange={handleWfmLoginChange} modCopiesMap={modCopiesMap} />
        </div>

        {/* ── Relics module ── */}
        {activeModule === "relics" && (
          <ErrorBoundary>
            <RelicHelper inventory={inventory} refreshKey={itemsRefreshKey} colorblindMode={colorblindMode} />
          </ErrorBoundary>
        )}

        {/* ── Rivens module ── */}
        {activeModule === "rivens" && (
          <ErrorBoundary>
            <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>
              <RivenAnalyzer />
            </div>
          </ErrorBoundary>
        )}

        {/* ── Arbitrations module ── */}
        {activeModule === "arbitrations" && (
          <ErrorBoundary>
            <Arbitrations
              favorites={arbFavorites}
              onToggleFavorite={id => setArbFavorites(prev =>
                prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]
              )}
              leadMins={arbLeadMins}
              onLeadChange={setArbLeadMins}
              permissionDenied={arbPermissionDenied}
              onPermissionChange={setArbPermissionDenied}
              tierFilter={arbTierFilter}
              onTierFilterChange={setArbTierFilter}
              alertTiers={arbAlertTiers}
              onAlertTiersChange={setArbAlertTiers}
              scheduleDays={arbScheduleDays}
              onScheduleDaysChange={setArbScheduleDays}
              clockFormat={clockFormat}
            />
          </ErrorBoundary>
        )}

        {/* ── Timers module ── */}
        {activeModule === "timers" && (
          <ErrorBoundary>
            <TimerHelper
              favorites={timerFavorites}
              onFavoriteToggle={id => setTimerFavorites(prev =>
                prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]
              )}
              fissureWatches={fissureWatches}
              onAddWatch={w => setFissureWatches(prev => [...prev, w])}
              onRemoveWatch={id => setFissureWatches(prev => prev.filter(w => w.id !== id))}
              fissureNotifications={fissureNotifications}
              onFissureNotificationsChange={setFissureNotifications}
              inventory={inventory}
            />
          </ErrorBoundary>
        )}

        {/* ── Statistics module ── */}
        {activeModule === "statistics" && (
          <ErrorBoundary>
            <Statistics clockFormat={clockFormat} />
          </ErrorBoundary>
        )}

        {/* ── Completionist module ── */}
        {activeModule === "completionist" && (
          <ErrorBoundary>
            <CompletionistTabs inventory={inventory} refreshKey={itemsRefreshKey} />
          </ErrorBoundary>
        )}

        </div>

        <ChangeLog
          changes={changeLog}
          arrivalToken={changeLogArrivalToken}
          lastScanAt={lastInventoryScanAt}
          catalog={catalog}
          clockFormat={clockFormat}
          onItemClick={openChangeLogItem}
          onChangeLogClick={openRecentChanges}
          onCategoryClick={openRecentCategory}
        />
        </div>

        {/* ── Modular Window — always visible unless popped out ── */}
        {!modularPopout && <ModularWindow
          tracked={tracked}
          onTrackedChange={setTracked}
          onUntrack={toggleTracked}
          favorites={favorites}
          onFavoritesChange={setFavorites}
          onUnfavorite={toggleFavorite}
          timerFavorites={timerFavorites}
          onTimerFavoritesChange={setTimerFavorites}
          onTimerUnfavorite={id => setTimerFavorites(prev => prev.filter(x => x !== id))}
          fissureWatches={fissureWatches}
          inventory={inventory}
          catalog={catalog}
          width={modularWidth}
          onWidthChange={setModularWidth}
          sectionOrder={modularSectionOrder}
          onSectionOrderChange={setModularSectionOrder}
        />}

      </div>
    </div>
    </ImgCacheDirContext.Provider>
  );
}
