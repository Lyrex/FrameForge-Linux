import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { mark } from "../startupMark";
import { TAURI_COMMANDS, TAURI_EVENTS } from "../constants/tauri";
import { refreshCatalog } from "./useCatalog";
import type { CatalogItem, CraftingJob, QuantityMap } from "../types/items";
import type { ChangeLogEntry, InventoryUpdate } from "../types/inventory";
import type { ItemListStatus } from "../types/tauri";

interface UseInventoryDataReturn {
  // State
  catalog: CatalogItem[];
  quantities: QuantityMap;
  scannerMods: Record<string, { total: number; by_rank: Record<string, number> }>;
  crafting: CraftingJob[];
  masteryRank: number | null;
  masteryData: Record<string, number>;
  ownedLevels: Record<string, number[]>;
  playerName: string | null;
  subsummedWarframes: Set<string>;
  archonShards: Record<string, { type: string; tauforged: boolean; color: string; boost?: string }[]>;
  formaData: Record<string, number>;
  inventoryReady: boolean;
  lastInventoryScanAt: number | null;
  changeLog: ChangeLogEntry[];
  changeLogArrivalToken: number;
  lastChanged: Record<string, number>;
  monitoring: boolean;
  warframeRunning: boolean;
  itemCount: number;
  recipeCount: number;
  fetching: boolean;
  fetchMsg: string;
  imgCacheDir: string;
  itemsRefreshKey: number;

  // Refs
  catalogRef: React.MutableRefObject<CatalogItem[]>;

  // Callbacks
  handleFetch: () => Promise<void>;

  // Setters
  setCatalog: React.Dispatch<React.SetStateAction<CatalogItem[]>>;
  setQuantities: React.Dispatch<React.SetStateAction<QuantityMap>>;
  setScannerMods: React.Dispatch<React.SetStateAction<Record<string, { total: number; by_rank: Record<string, number> }>>>;
  setCrafting: React.Dispatch<React.SetStateAction<CraftingJob[]>>;
  setMasteryData: React.Dispatch<React.SetStateAction<Record<string, number>>>;
  setArchonShards: React.Dispatch<React.SetStateAction<Record<string, { type: string; tauforged: boolean; color: string; boost?: string }[]>>>;
  setFormaData: React.Dispatch<React.SetStateAction<Record<string, number>>>;
  setChangeLog: React.Dispatch<React.SetStateAction<ChangeLogEntry[]>>;
  setLastChanged: React.Dispatch<React.SetStateAction<Record<string, number>>>;
  setItemsRefreshKey: React.Dispatch<React.SetStateAction<number>>;
  setMonitoring: React.Dispatch<React.SetStateAction<boolean>>;
  setWarframeRunning: React.Dispatch<React.SetStateAction<boolean>>;
  setPlayerName: React.Dispatch<React.SetStateAction<string | null>>;
}

export function useInventoryData(): UseInventoryDataReturn {
  // ── State ───────────────────────────────────────────────────────────────────
  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  const [quantities, setQuantities] = useState<QuantityMap>({});
  const [scannerMods, setScannerMods] = useState<Record<string, { total: number; by_rank: Record<string, number> }>>({});
  const [crafting, setCrafting] = useState<CraftingJob[]>([]);
  const [masteryRank, setMasteryRank] = useState<number | null>(null);
  const [masteryData, setMasteryData] = useState<Record<string, number>>({});
  const [ownedLevels, setOwnedLevels] = useState<Record<string, number[]>>({});
  const [playerName, setPlayerName] = useState<string | null>(null);
  const [subsummedWarframes, setSubsummedWarframes] = useState<Set<string>>(new Set());
  const [archonShards, setArchonShards] = useState<Record<string, { type: string; tauforged: boolean; color: string; boost?: string }[]>>({});
  const [formaData, setFormaData] = useState<Record<string, number>>({});
  const [inventoryReady, setInventoryReady] = useState(false);
  const [lastInventoryScanAt, setLastInventoryScanAt] = useState<number | null>(null);
  const [changeLog, setChangeLog] = useState<ChangeLogEntry[]>([]);
  const [changeLogArrivalToken, setChangeLogArrivalToken] = useState(0);
  const [lastChanged, setLastChanged] = useState<Record<string, number>>({});
  const [monitoring, setMonitoring] = useState(false);
  const [warframeRunning, setWarframeRunning] = useState(false);
  const [itemCount, setItemCount] = useState(0);
  const [recipeCount, setRecipeCount] = useState(0);
  const [fetching, setFetching] = useState(false);
  const [fetchMsg, setFetchMsg] = useState("");
  const [imgCacheDir, setImgCacheDir] = useState("");
  const [itemsRefreshKey, setItemsRefreshKey] = useState(0);

  // ── Refs ────────────────────────────────────────────────────────────────────
  const inventoryReadyRef = useRef(false);
  const catalogRef = useRef<CatalogItem[]>([]);

  const reloadCatalog = async () => {
    const items = await invoke<CatalogItem[]>(TAURI_COMMANDS.GET_ALL_ITEMS);
    setCatalog(items);
    catalogRef.current = items;
    mark("catalog state set");
    const status = await invoke<ItemListStatus>("get_item_list_status");
    setItemCount(status.count);
    setRecipeCount(status.recipe_count);
    setItemsRefreshKey(k => k + 1);
    void refreshCatalog();
    invoke("prewarm_image_cache").catch(() => {});
    return status;
  };

  // ── Fetch item list ─────────────────────────────────────────────────────────
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
      const status = await reloadCatalog();
      setFetchMsg(`Loaded ${count.toLocaleString()} items, ${status.recipe_count.toLocaleString()} recipes`);
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

  // ── Bootstrap ───────────────────────────────────────────────────────────────
  useEffect(() => {
    invoke<string[]>("get_saved_consumed_suits")
      .then(suits => { if (suits.length > 0) setSubsummedWarframes(new Set(suits)); })
      .catch(() => {});

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

    invoke<string>("get_img_cache_dir").then(setImgCacheDir).catch(() => {});
    invoke("prewarm_image_cache").catch(() => {});
  }, []);

  // ── Background catalogue refresh ────────────────────────────────────────────
  // The Rust side rebuilds the catalogue on its own (first run after an upgrade,
  // daily refresh). Reload what the UI holds so no manual "Refresh item list" is needed.
  useEffect(() => {
    const unlisten = listen<number>(TAURI_EVENTS.CATALOGUE_UPDATED, () => {
      reloadCatalog().catch(() => {
        /* keep the current catalogue; the manual refresh button still works */
      });
    });
    return () => { unlisten.then(fn => fn()); };
  }, []); // eslint-disable-line

  // ── Inventory update events ─────────────────────────────────────────────────
  useEffect(() => {
    const unlisten = listen<InventoryUpdate>(TAURI_EVENTS.INVENTORY_UPDATE, (e) => {
      const p = e.payload;
      setLastInventoryScanAt(p.scanned_at);
      if (!inventoryReadyRef.current) {
        inventoryReadyRef.current = true;
        setInventoryReady(true);
      }
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
            if (/^\d+$/.test(raw)) {
              const n = parseInt(raw);
              raw = INT_TO_ACC[n % 5] ?? raw;
            }
            const tauforged = raw.includes("MYTHIC") || raw.includes("TAU") || parseInt(s.color) >= 5;
            const entry = SHARD_COLORS.find(e => raw.startsWith(e.prefix));
            const colorInfo = entry ?? { type: "Unknown", colorHex: "#b0b0b0", tauHex: "#d0d0d0" };
            const seg = s.upgrade_type.split("/").pop() ?? "";
            const boostRaw = seg.replace(/^ArchonCrystalUpgrade(?:Warframe|Companion)?/, "");
            const boost = boostRaw.replace(/([A-Z])/g, " $1").trim() || undefined;
            const color = tauforged ? colorInfo.tauHex : colorInfo.colorHex;
            return { type: colorInfo.type, tauforged, color, boost };
          });
        }
        if (p.is_full_pass) {
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

  // ── Player name (immediate, from EE.log "Logged in NAME") ──────────────────
  useEffect(() => {
    const unlisten = listen<string>("player-name", e => {
      setPlayerName(e.payload);
    });
    return () => { unlisten.then(fn => fn()); };
  }, []);

  return {
    // State
    catalog,
    quantities,
    scannerMods,
    crafting,
    masteryRank,
    masteryData,
    ownedLevels,
    playerName,
    subsummedWarframes,
    archonShards,
    formaData,
    inventoryReady,
    lastInventoryScanAt,
    changeLog,
    changeLogArrivalToken,
    lastChanged,
    monitoring,
    warframeRunning,
    itemCount,
    recipeCount,
    fetching,
    fetchMsg,
    imgCacheDir,
    itemsRefreshKey,

    // Refs
    catalogRef,

    // Callbacks
    handleFetch,

    // Setters
    setCatalog,
    setQuantities,
    setScannerMods,
    setCrafting,
    setMasteryData,
    setArchonShards,
    setFormaData,
    setChangeLog,
    setLastChanged,
    setItemsRefreshKey,
    setMonitoring,
    setWarframeRunning,
    setPlayerName,
  };
}
