import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { mark } from "./startupMark";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { applyScale } from "./lib/uiScale";
import { useContextMenu, CtxMenu } from "./shared/CtxMenu";
import { extractItemName } from "./lib/itemContext";
import { matchesSearchTerms, splitSearchTerms } from "./lib/search";
import { openWiki, copyWikiLink } from "./lib/wiki";
import { useModularWindow } from "./hooks/useModularWindow";
import { useSettings } from "./hooks/useSettings";
import { useInventoryData } from "./hooks/useInventoryData";
import { useOverlays } from "./hooks/useOverlays";
import { useTimerPreferences } from "./hooks/useTimerPreferences";
import { useFissureNotifications } from "./hooks/useFissureNotifications";
import { CATEGORIES } from "./constants/categories";

import { getCurrentWindow } from "@tauri-apps/api/window";

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
import { notify } from "./lib/notify";
import { runAlertPass, DEFAULT_LEAD_MINS, EVAL_INTERVAL_MS, type AlertRule, type ScheduleEntry } from "./arbitration/arbitrationAlerts";
import { useArbitrationSchedule } from "./arbitration/arbitrationSchedule";
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
import KeepMountedWhenHidden from "./KeepMountedWhenHidden";
import { FOUNDRY_FILTERS_DEFAULT, INVENTORY_FILTERS_DEFAULT, MARKET_FILTERS_DEFAULT, RELIC_FILTERS_DEFAULT } from "./constants/filters";
import { PREFERENCE_KEYS } from "./constants/preferences";
import { TAURI_COMMANDS, TAURI_EVENTS } from "./constants/tauri";
import type { FoundryFilters, InventoryFilters, MarketFilters, RelicFilters } from "./types/filters";
import { type FilterPresetModule } from "./types/filterPresets";
import type { ViewMode } from "./types/ui";
import type { CatalogItem, CraftingJob, InventoryItem } from "./types/items";
import type { ChangeLogEntry, ModCopy } from "./types/inventory";
import type { BlobStatusPayload, SettingsFile, SettingsPatch, WfmCredentials, WfmSession } from "./types/tauri";
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

// ─── App ──────────────────────────────────────────────────────────────────────

// RelicAndRivenTab is kept but now just shows RelicHelper — Rivens moved to own tab

// Render-time, so it fires even if the first commit never happens.
let firstRenderMarked = false;
let renderBodyMarked = false;

export default function App() {
  // If we're the overlay window, render only the overlay UI
  if (IS_OVERLAY) return <Overlay />;
  if (IS_RIVEN_OVERLAY) return <RivenOverlayWindow />;
  if (IS_RELIC_PICK_OVERLAY) return <RelicPickOverlay />;
  if (IS_ARBITRATION_OVERLAY) return <ArbitrationOverlay />;
  if (!firstRenderMarked) {
    firstRenderMarked = true;
    mark("App first render");
  }
  useEffect(() => {
    mark("App mounted");
  }, []);
  // If we're the pop-out modular window, render the standalone modular UI
  if (IS_MODULAR) return <ModularWindowPage />;

  const [activeModule, setActiveModule] = useState<Module>("inventory");
  const [visitedModules, setVisitedModules] = useState<Set<Module>>(() => new Set(["inventory"]));
  const activateModule = useCallback((module: Module) => {
    setVisitedModules(previous => previous.has(module) ? previous : new Set([...previous, module]));
    setActiveModule(module);
  }, []);
  const { ctxMenu, open: openCtx, close: closeCtx } = useContextMenu();

  // ── Custom hooks ──────────────────────────────────────────────────────────
  const inv = useInventoryData();
  const settings = useSettings(inv.setMonitoring);
  const modular = useModularWindow();
  const timerPreferences = useTimerPreferences();

  const {
    memoryScannerEnabled, setMemoryScannerEnabled,
    blobLogEnabled, setBlobLogEnabled,
    autoDiagEnabled, setAutoDiagEnabled,
    overlayEnabled, setOverlayEnabled,
    overlayPriority, setOverlayPriority,
    textScale, setTextScale,
    colorblindMode, setColorblindMode,
    clockFormat, setClockFormat,
    foundryPageSize, setFoundryPageSize,
    relicPickEnabled, setRelicPickEnabled,
    memTriggerEnabled, setMemTriggerEnabled,
    relicPickPriority, setRelicPickPriority,
    relicPickRefinement,
    relicPickLines, setRelicPickLines,
    masteryExclude, setMasteryExclude,
    wfmInvisibleOnStart, setWfmInvisibleOnStart,
    wfmInvisibleOnClose, setWfmInvisibleOnClose,
    wfmAutoInvisible, setWfmAutoInvisible,
    wfmAutoInvisibleMins, setWfmAutoInvisibleMins,
    arbFavorites, setArbFavorites,
    arbLeadMins, setArbLeadMins,
    arbTierFilter, setArbTierFilter,
    arbAlertTiers, setArbAlertTiers,
    arbScheduleDays, setArbScheduleDays,
    arbOverlayEnabled, setArbOverlayEnabled,
    filterPresets, setFilterPresets,
    settingsLoadedRef, settingsRef,
    wfmInvisibleOnStartRef, wfmInvisibleOnCloseRef, arbFiredRef,
    saveAllSettings, loadSettings,
  } = settings;

  const { tracked, favorites, modularWidth, modularSectionOrder, modularPopout, toggleTracked, toggleFavorite, applySettings: applyModularSettings, setTracked, setFavorites, setModularWidth, setModularSectionOrder, setModularPopout } = modular;
  const { timerFavorites, fissureWatches, fissureNotifications, applySettings: applyTimerSettings, setTimerFavorites, setFissureWatches, setFissureNotifications } = timerPreferences;
  useFissureNotifications(fissureWatches, fissureNotifications);

  const {
    catalog, quantities, scannerMods,
    crafting, masteryRank, masteryData, ownedLevels, playerName,
    subsummedWarframes, archonShards, formaData,
    inventoryReady, lastInventoryScanAt, changeLog, changeLogArrivalToken,
    lastChanged, monitoring, warframeRunning, itemCount, recipeCount,
    fetching, fetchMsg, imgCacheDir, itemsRefreshKey,
    handleFetch,
    setQuantities,
    setScannerMods, setMasteryData, setArchonShards,
    setFormaData, setChangeLog, setLastChanged,
    setItemsRefreshKey,
  } = inv;

  const overlays = useOverlays(setQuantities);
  const { overlayStatus } = overlays;

  // ── Remaining local state (not in any hook) ──────────────────────────────
  const [poking, setPoking] = useState(false);
  const [updateAvailable, setUpdateAvailable] = useState<UpdateAvailable | null>(null);
  const [showUpdateDialog, setShowUpdateDialog] = useState(false);
  const [wfmLoggedIn, setWfmLoggedIn] = useState(false);
  const wfmLoggedInRef = useRef(false);
  const [inventoryFilters, setInventoryFilters] = useState<InventoryFilters>(INVENTORY_FILTERS_DEFAULT);
  const [foundryFilters, setFoundryFilters] = useState<FoundryFilters>(FOUNDRY_FILTERS_DEFAULT);
  const [marketFilters, setMarketFilters] = useState<MarketFilters>(MARKET_FILTERS_DEFAULT);
  const [relicFilters, setRelicFilters] = useState<RelicFilters>(RELIC_FILTERS_DEFAULT);
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
  const [showSettings, setShowSettings] = useState(false);
  const [settingsTab, setSettingsTab] = useState<'general' | 'overlays' | 'market' | 'filters' | 'accessibility' | 'data' | 'debugging'>('general');
  const [settingsFilterModule, setSettingsFilterModule] = useState<FilterPresetModule>("inventory");
  const openFilterSettings = useCallback((module: FilterPresetModule) => {
    setSettingsFilterModule(module);
    setSettingsTab("filters");
    setShowSettings(true);
  }, []);
  const [appVersion, setAppVersion] = useState("");
  const [showInventoryBatchPreview, setShowInventoryBatchPreview] = useState(false);
  // "scanning" while blob capture is running, "done" briefly after it finishes
  const [blobStage, setBlobStage] = useState<"scanning" | "done" | null>(null);
  const blobDoneTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleWfmLoginChange = useCallback((loggedIn: boolean) => {
    setWfmLoggedIn(loggedIn);
    wfmLoggedInRef.current = loggedIn;
  }, []);

  settingsRef.current = { overlayEnabled, overlayPriority, textScale, colorblindMode, clockFormat, memoryScannerEnabled, blobLogEnabled, autoDiagEnabled, tracked, favorites, timerFavorites, fissureWatches, fissureNotifications, arbitrationFavorites: arbFavorites, arbitrationLeadMins: arbLeadMins, arbitrationOverlayEnabled: arbOverlayEnabled, arbitrationTierFilter: arbTierFilter, arbitrationAlertTiers: arbAlertTiers, arbitrationScheduleDays: arbScheduleDays, modularWidth, modularSectionOrder, modularPopout, wfmInvisibleOnStart, wfmInvisibleOnClose, wfmAutoInvisible, wfmAutoInvisibleMins, relicPickEnabled, relicPickPriority, relicPickRefinement, relicPickLines, foundryPageSize, memTriggerEnabled, masteryExclude, filterPresets };

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
    loadSettings().then(settings => {
      if (settings) {
        applyModularSettings(settings);
        applyTimerSettings(settings);
        settingsLoadedRef.current = true;
      }
    });
    getVersion().then(setAppVersion).catch(() => {});
  }, [loadSettings, applyModularSettings, applyTimerSettings]);

  useEffect(() => {
    const unlisten = listen(TAURI_EVENTS.SETTINGS_UPDATED, () => {
      invoke<string>(TAURI_COMMANDS.LOAD_SETTINGS).then(json => {
        if (!json) return;
        try {
          const updated = JSON.parse(json) as SettingsFile;
          applyModularSettings(updated);
          applyTimerSettings(updated);
        } catch {}
      }).catch(() => {});
    });
    return () => { unlisten.then(fn => fn()); };
  }, [applyModularSettings, applyTimerSettings]);

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

  useEffect(() => {
    if (settingsLoadedRef.current) saveAllSettings();
  }, [tracked, favorites, timerFavorites, fissureWatches, fissureNotifications, arbFavorites, arbLeadMins, arbTierFilter, arbAlertTiers, arbScheduleDays, memoryScannerEnabled, blobLogEnabled, autoDiagEnabled, modularSectionOrder, modularPopout, filterPresets]); // eslint-disable-line

  // ── Arbitration alerts ─────────────────────────────────────────────────────
  //
  // Here rather than in Arbitrations, for the same reason the fissure alerts
  // are: that module is not mounted until the user first opens it.

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

  // TODO: move into Arbitrations. Only that module reads or writes it, and the
  // module stays mounted once visited, so nothing here needs to hold it.
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
  }, [arbAlertsOn]); // eslint-disable-line

  const commitModularWidth = useCallback((width: number) => {
    const patch: SettingsPatch = { modularWidth: width };
    invoke(TAURI_COMMANDS.SAVE_SETTINGS, { json: JSON.stringify(patch) }).catch(() => {});
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
    const searchTerms = splitSearchTerms(search);
    // Changelog order map: lower index = more recent position in changelog
    const changeOrder = new Map<string, number>();
    changeLog.forEach((c, i) => { if (!changeOrder.has(c.unique_name)) changeOrder.set(c.unique_name, i); });
    const out: (CatalogItem & { qty: number })[] = [];
    for (const i of catalog) {
      if (i.name === "Blueprint") continue;
      if (category !== "all" && i.category !== category) continue;
      if (!matchesSearchTerms(searchTerms, i.name)) continue;
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
    activateModule("inventory");
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
    activateModule("inventory");
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

  if (!renderBodyMarked) {
    renderBodyMarked = true;
    mark("App render body done");
  }
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
                  onClick={!wfmLoggedIn ? () => activateModule("market") : undefined}
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

      {showSettings && <SettingsModal onClose={() => setShowSettings(false)} {...{ settingsTab, setSettingsTab, settingsFilterModule, setSettingsFilterModule, filterPresets, setFilterPresets, inventoryFilters, setInventoryFilters, foundryFilters, setFoundryFilters, marketFilters, setMarketFilters, relicFilters, setRelicFilters, foundryPageSize, setFoundryPageSize, settingsRef, saveAllSettings, memoryScannerEnabled, setMemoryScannerEnabled, modularPopout, setModularPopout, overlayStatus, overlayEnabled, setOverlayEnabled, overlayPriority, setOverlayPriority, memTriggerEnabled, setMemTriggerEnabled, relicPickEnabled, setRelicPickEnabled, relicPickPriority, setRelicPickPriority, relicPickLines, setRelicPickLines, masteryExclude, setMasteryExclude, wfmLoggedIn, wfmInvisibleOnStart, setWfmInvisibleOnStart, wfmInvisibleOnStartRef, wfmInvisibleOnClose, setWfmInvisibleOnClose, wfmInvisibleOnCloseRef, wfmAutoInvisible, setWfmAutoInvisible, wfmAutoInvisibleMins, setWfmAutoInvisibleMins, colorblindMode, setColorblindMode, textScale, setTextScale, clockFormat, setClockFormat, itemCount, recipeCount, handleFetch, fetching, fetchMsg, setQuantities, setScannerMods, setMasteryData, setArchonShards, setFormaData, setChangeLog, setLastChanged, setItemsRefreshKey, blobLogEnabled, setBlobLogEnabled, setShowInventoryBatchPreview, autoDiagEnabled, setAutoDiagEnabled, appVersion, arbOverlayEnabled, setArbOverlayEnabled }} onUpdateFound={u => { setUpdateAvailable(u); setShowUpdateDialog(true); }} />}
      {showUpdateDialog && updateAvailable && (
        <UpdateDialog update={updateAvailable} onDismiss={() => setShowUpdateDialog(false)} />
      )}

      {showInventoryBatchPreview && <InventoryBatchPreview onClose={closeInventoryBatchPreview} />}

      <div className="body">

        <AppNavigation activeModule={activeModule} onModuleChange={activateModule} />

        <div className="app-content">
        <div className="module-content">
        {/* ── Inventory module ── */}
        {visitedModules.has("inventory") && (
        <KeepMountedWhenHidden active={activeModule === "inventory"}>
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
                filterPresets={filterPresets}
                onFilterPresetsChange={setFilterPresets}
                onOpenSettings={openFilterSettings}
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
        </KeepMountedWhenHidden>
        )}

        {ctxMenu && <CtxMenu state={ctxMenu} onClose={closeCtx} />}

        {/* ── Foundry module ── */}
        {visitedModules.has("foundry") && (
        <KeepMountedWhenHidden active={activeModule === "foundry"}>
          <ErrorBoundary>
            <Foundry inventory={inventory} refreshKey={itemsRefreshKey} crafting={crafting} filters={foundryFilters} onFiltersChange={setFoundryFilters} filterPresets={filterPresets} onFilterPresetsChange={setFilterPresets} onOpenSettings={openFilterSettings} colorblindMode={colorblindMode} subsummedWarframes={subsummedWarframes} tracked={tracked} onTrackToggle={toggleTracked} pageSize={foundryPageSize} />
          </ErrorBoundary>
        </KeepMountedWhenHidden>
        )}

        {/* ── Market Helper module ── */}
        {/* Keep mounted at all times so WfmTrading's trade-completed listener
            (auto listing update) fires regardless of which tab is active. */}
        <KeepMountedWhenHidden active={activeModule === "market"}>
          <MarketHelper inventory={inventory} refreshKey={itemsRefreshKey} crafting={crafting} filters={marketFilters} onFiltersChange={setMarketFilters} filterPresets={filterPresets} onFilterPresetsChange={setFilterPresets} onOpenSettings={openFilterSettings} onWfmLoginChange={handleWfmLoginChange} modCopiesMap={modCopiesMap} />
        </KeepMountedWhenHidden>

        {/* ── Relics module ── */}
        {visitedModules.has("relics") && (
        <KeepMountedWhenHidden active={activeModule === "relics"}>
          <ErrorBoundary>
            <RelicHelper inventory={inventory} filters={relicFilters} onFiltersChange={setRelicFilters} filterPresets={filterPresets} onFilterPresetsChange={setFilterPresets} onOpenSettings={openFilterSettings} colorblindMode={colorblindMode} />
          </ErrorBoundary>
        </KeepMountedWhenHidden>
        )}

        {/* ── Rivens module ── */}
        {visitedModules.has("rivens") && (
        <KeepMountedWhenHidden active={activeModule === "rivens"}>
          <ErrorBoundary>
            <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>
              <RivenAnalyzer wfmLoggedIn={wfmLoggedIn} />
            </div>
          </ErrorBoundary>
        </KeepMountedWhenHidden>
        )}

        {/* ── Arbitrations module ── */}
        {visitedModules.has("arbitrations") && (
        <KeepMountedWhenHidden active={activeModule === "arbitrations"}>
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
        </KeepMountedWhenHidden>
        )}

        {/* ── Timers module ── */}
        {visitedModules.has("timers") && (
        <KeepMountedWhenHidden active={activeModule === "timers"}>
          <ErrorBoundary>
            <TimerHelper
              active={activeModule === "timers"}
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
        </KeepMountedWhenHidden>
        )}

        {/* ── Statistics module ── */}
        {visitedModules.has("statistics") && (
        <KeepMountedWhenHidden active={activeModule === "statistics"}>
          <ErrorBoundary>
            <Statistics clockFormat={clockFormat} />
          </ErrorBoundary>
        </KeepMountedWhenHidden>
        )}

        {/* ── Completionist module ── */}
        {visitedModules.has("completionist") && (
        <KeepMountedWhenHidden active={activeModule === "completionist"}>
          <ErrorBoundary>
            <CompletionistTabs inventory={inventory} refreshKey={itemsRefreshKey} clockFormat={clockFormat} tracked={tracked} onTrackToggle={toggleTracked} />
          </ErrorBoundary>
        </KeepMountedWhenHidden>
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
          onWidthCommit={commitModularWidth}
          sectionOrder={modularSectionOrder}
          onSectionOrderChange={setModularSectionOrder}
        />}

      </div>
    </div>
    </ImgCacheDirContext.Provider>
  );
}
