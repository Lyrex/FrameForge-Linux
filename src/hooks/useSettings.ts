import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PREFERENCE_KEYS } from "../constants/preferences";
import { TAURI_COMMANDS } from "../constants/tauri";
import {
  CLOCK_FORMAT_OPTIONS,
  DEFAULT_CLOCK_FORMAT,
  DEFAULT_FOUNDRY_PAGE_SIZE,
  DEFAULT_MASTERY_EXCLUDE,
  DEFAULT_RELIC_OVERLAY_PRIORITY,
  DEFAULT_RELIC_PICK_LINES,
  DEFAULT_RELIC_PICK_PRIORITY,
  DEFAULT_RELIC_PICK_REFINEMENT,
  FOUNDRY_PAGE_SIZE_OPTIONS,
  MASTERY_EXCLUDE_OPTIONS,
  RELIC_PICK_LINES_OPTIONS,
  RELIC_PICK_PRIORITY_OPTIONS,
  RELIC_PICK_REFINEMENT_OPTIONS,
} from "../constants/settings";
import { clampLead, DEFAULT_LEAD_MINS } from "../arbitration/arbitrationAlerts";
import { clampScheduleDays, DEFAULT_SCHEDULE_DAYS } from "../arbitration/arbitrationSchedule";
import { sanitizeTierKeys, TIER_KEYS, type TierKey } from "../arbitration/arbitrationTiers";
import type { ClockFormat } from "../lib/clockFormat";
import type { FissureWatch, FoundryPageSize, MasteryExclude, RelicOverlayPriority, RelicPickLines, RelicPickPriority, RelicRefinement, SettingsSnapshot } from "../types/settings";
import type { FilterPresetSettings } from "../types/filterPresets";
import { parseFilterPresetSettings } from "../types/filterPresets";
import type { SettingsFile, SettingsPatch } from "../types/tauri";

interface UseSettingsReturn {
  // Settings state
  memoryScannerEnabled: boolean;
  blobLogEnabled: boolean;
  autoDiagEnabled: boolean;
  overlayEnabled: boolean;
  overlayPriority: RelicOverlayPriority;
  textScale: number;
  colorblindMode: boolean;
  clockFormat: ClockFormat;
  foundryPageSize: FoundryPageSize;
  relicPickEnabled: boolean;
  memTriggerEnabled: boolean;
  relicPickPriority: RelicPickPriority;
  relicPickRefinement: RelicRefinement;
  relicPickLines: RelicPickLines;
  masteryExclude: MasteryExclude;
  wfmInvisibleOnStart: boolean;
  wfmInvisibleOnClose: boolean;
  wfmAutoInvisible: boolean;
  wfmAutoInvisibleMins: number;
  arbFavorites: string[];
  arbLeadMins: number;
  arbTierFilter: TierKey[];
  arbAlertTiers: TierKey[];
  arbScheduleDays: number;
  arbOverlayEnabled: boolean;
  filterPresets: FilterPresetSettings;

  // Setters
  setMemoryScannerEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setBlobLogEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setAutoDiagEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setOverlayEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setOverlayPriority: React.Dispatch<React.SetStateAction<RelicOverlayPriority>>;
  setTextScale: React.Dispatch<React.SetStateAction<number>>;
  setColorblindMode: React.Dispatch<React.SetStateAction<boolean>>;
  setClockFormat: React.Dispatch<React.SetStateAction<ClockFormat>>;
  setFoundryPageSize: React.Dispatch<React.SetStateAction<FoundryPageSize>>;
  setRelicPickEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setMemTriggerEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setRelicPickPriority: React.Dispatch<React.SetStateAction<RelicPickPriority>>;
  setRelicPickRefinement: React.Dispatch<React.SetStateAction<RelicRefinement>>;
  setRelicPickLines: React.Dispatch<React.SetStateAction<RelicPickLines>>;
  setMasteryExclude: React.Dispatch<React.SetStateAction<MasteryExclude>>;
  setWfmInvisibleOnStart: React.Dispatch<React.SetStateAction<boolean>>;
  setWfmInvisibleOnClose: React.Dispatch<React.SetStateAction<boolean>>;
  setWfmAutoInvisible: React.Dispatch<React.SetStateAction<boolean>>;
  setWfmAutoInvisibleMins: React.Dispatch<React.SetStateAction<number>>;
  setArbFavorites: React.Dispatch<React.SetStateAction<string[]>>;
  setArbLeadMins: React.Dispatch<React.SetStateAction<number>>;
  setArbTierFilter: React.Dispatch<React.SetStateAction<TierKey[]>>;
  setArbAlertTiers: React.Dispatch<React.SetStateAction<TierKey[]>>;
  setArbScheduleDays: React.Dispatch<React.SetStateAction<number>>;
  setArbOverlayEnabled: React.Dispatch<React.SetStateAction<boolean>>;
  setFilterPresets: React.Dispatch<React.SetStateAction<FilterPresetSettings>>;

  // Refs
  settingsLoadedRef: React.MutableRefObject<boolean>;
  settingsRef: React.MutableRefObject<SettingsSnapshot>;
  wfmInvisibleOnStartRef: React.MutableRefObject<boolean>;
  wfmInvisibleOnCloseRef: React.MutableRefObject<boolean>;
  // Persisted, so a restart inside the lead window does not alert a second
  // time for the same hour.
  arbFiredRef: React.MutableRefObject<string[]>;

  // Callbacks
  saveAllSettings: () => void;
  loadSettings: () => Promise<SettingsFile | null>;
}

export function useSettings(
  setMonitoring: React.Dispatch<React.SetStateAction<boolean>>,
): UseSettingsReturn {
  // ── Settings state ──────────────────────────────────────────────────────────
  const [memoryScannerEnabled, setMemoryScannerEnabled] = useState(false);
  const [blobLogEnabled, setBlobLogEnabled] = useState(false);
  const [autoDiagEnabled, setAutoDiagEnabled] = useState(false);
  const [overlayEnabled, setOverlayEnabled] = useState<boolean>(
    () => localStorage.getItem(PREFERENCE_KEYS.OVERLAY_ENABLED) !== "false"
  );
  const [overlayPriority, setOverlayPriority] = useState<RelicOverlayPriority>(
    () => (localStorage.getItem(PREFERENCE_KEYS.OVERLAY_PRIORITY) ?? DEFAULT_RELIC_OVERLAY_PRIORITY) as RelicOverlayPriority
  );
  const [textScale, setTextScale] = useState(() => {
    const s = parseFloat(localStorage.getItem(PREFERENCE_KEYS.TEXT_SCALE) ?? "1");
    document.documentElement.style.setProperty("--ff-scale", s.toString());
    return s;
  });
  const [colorblindMode, setColorblindMode] = useState(() =>
    localStorage.getItem(PREFERENCE_KEYS.COLORBLIND_MODE) === "true"
  );
  const [clockFormat, setClockFormat] = useState<ClockFormat>(DEFAULT_CLOCK_FORMAT);
  const [foundryPageSize, setFoundryPageSize] = useState<FoundryPageSize>(DEFAULT_FOUNDRY_PAGE_SIZE);
  const [relicPickEnabled, setRelicPickEnabled] = useState<boolean>(true);
  const [memTriggerEnabled, setMemTriggerEnabled] = useState<boolean>(false);
  const [relicPickPriority, setRelicPickPriority] = useState<RelicPickPriority>(DEFAULT_RELIC_PICK_PRIORITY);
  const [relicPickRefinement, setRelicPickRefinement] = useState<RelicRefinement>(DEFAULT_RELIC_PICK_REFINEMENT);
  const [relicPickLines, setRelicPickLines] = useState<RelicPickLines>(DEFAULT_RELIC_PICK_LINES);
  const [masteryExclude, setMasteryExclude] = useState<MasteryExclude>(DEFAULT_MASTERY_EXCLUDE);
  const [wfmInvisibleOnStart, setWfmInvisibleOnStart] = useState(false);
  const [wfmInvisibleOnClose, setWfmInvisibleOnClose] = useState(false);
  const [wfmAutoInvisible, setWfmAutoInvisible] = useState(false);
  const [wfmAutoInvisibleMins, setWfmAutoInvisibleMins] = useState(30);
  const [arbFavorites, setArbFavorites] = useState<string[]>([]);
  const [arbLeadMins, setArbLeadMins] = useState(DEFAULT_LEAD_MINS);
  // The filter starts wide and the alert rule starts empty: showing every hour
  // is what a browser is for, while alerting is opt-in.
  const [arbTierFilter, setArbTierFilter] = useState<TierKey[]>([...TIER_KEYS]);
  const [arbAlertTiers, setArbAlertTiers] = useState<TierKey[]>([]);
  const [arbScheduleDays, setArbScheduleDays] = useState(DEFAULT_SCHEDULE_DAYS);
  const [arbOverlayEnabled, setArbOverlayEnabled] = useState<boolean>(false);
  const [filterPresets, setFilterPresets] = useState<FilterPresetSettings>(() => parseFilterPresetSettings(undefined));

  // ── Refs ────────────────────────────────────────────────────────────────────
  const settingsLoadedRef = useRef(false);
  const settingsRef = useRef<SettingsSnapshot>({
    overlayEnabled: true,
    overlayPriority: DEFAULT_RELIC_OVERLAY_PRIORITY,
    textScale: 1,
    colorblindMode: false,
    clockFormat: DEFAULT_CLOCK_FORMAT,
    memoryScannerEnabled: false,
    blobLogEnabled: false,
    autoDiagEnabled: false,
    tracked: [] as string[],
    favorites: [] as string[],
    timerFavorites: [] as string[],
    fissureWatches: [] as FissureWatch[],
    fissureNotifications: true,
    arbitrationFavorites: [] as string[],
    arbitrationLeadMins: DEFAULT_LEAD_MINS,
    arbitrationOverlayEnabled: false,
    arbitrationTierFilter: [...TIER_KEYS] as TierKey[],
    arbitrationAlertTiers: [] as TierKey[],
    arbitrationScheduleDays: DEFAULT_SCHEDULE_DAYS,
    modularWidth: 240,
    modularSectionOrder: ["tracking", "favorites", "timers"] as string[],
    modularPopout: false,
    wfmInvisibleOnStart: false,
    wfmInvisibleOnClose: false,
    wfmAutoInvisible: false,
    wfmAutoInvisibleMins: 30,
    relicPickEnabled: true,
    relicPickPriority: DEFAULT_RELIC_PICK_PRIORITY,
    relicPickRefinement: DEFAULT_RELIC_PICK_REFINEMENT,
    relicPickLines: DEFAULT_RELIC_PICK_LINES,
    foundryPageSize: DEFAULT_FOUNDRY_PAGE_SIZE,
    memTriggerEnabled: false,
    masteryExclude: DEFAULT_MASTERY_EXCLUDE,
    filterPresets: { presets: [], restorePreviousFiltersOnPresetClick: false },
  });
  const wfmInvisibleOnStartRef = useRef(false);
  const wfmInvisibleOnCloseRef = useRef(false);
  const arbFiredRef = useRef<string[]>([]);

  // ── Save settings ───────────────────────────────────────────────────────────
  const saveAllSettings = useCallback(() => {
    if (!settingsLoadedRef.current) {
      console.error("save_settings skipped: settings not loaded yet, saving now would clobber the file");
      return;
    }
    const settings: SettingsPatch = { ...settingsRef.current };
    invoke(TAURI_COMMANDS.SAVE_SETTINGS, { json: JSON.stringify(settings) }).catch((e) => {
      console.error("save_settings failed:", e);
    });
  }, []);

  // ── Load settings from file ─────────────────────────────────────────────────
  const loadSettings = useCallback(async (): Promise<SettingsFile | null> => {
    try {
      const json = await invoke<string>(TAURI_COMMANDS.LOAD_SETTINGS);
      if (!json) { settingsLoadedRef.current = true; return null; }
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
        if (FOUNDRY_PAGE_SIZE_OPTIONS.includes(s.foundryPageSize)) {
          setFoundryPageSize(s.foundryPageSize);
        }
        if (typeof s.relicPickEnabled === "boolean") {
          setRelicPickEnabled(s.relicPickEnabled);
          invoke(TAURI_COMMANDS.SET_RELIC_PICK_ENABLED, { enabled: s.relicPickEnabled });
        }
        if (typeof s.memTriggerEnabled === "boolean") {
          setMemTriggerEnabled(s.memTriggerEnabled);
          invoke(TAURI_COMMANDS.SET_MEM_TRIGGER_ENABLED, { enabled: s.memTriggerEnabled });
        }
        if (RELIC_PICK_PRIORITY_OPTIONS.includes(s.relicPickPriority)) setRelicPickPriority(s.relicPickPriority);
        if (RELIC_PICK_REFINEMENT_OPTIONS.includes(s.relicPickRefinement)) setRelicPickRefinement(s.relicPickRefinement);
        if (RELIC_PICK_LINES_OPTIONS.includes(s.relicPickLines)) setRelicPickLines(s.relicPickLines);
        if (typeof s.masteryExclude === "object" && s.masteryExclude != null) {
          const stored = s.masteryExclude as Partial<MasteryExclude>;
          setMasteryExclude(Object.fromEntries(MASTERY_EXCLUDE_OPTIONS.map(o =>
            [o.key, typeof stored[o.key] === "boolean" ? stored[o.key] : DEFAULT_MASTERY_EXCLUDE[o.key]])) as MasteryExclude);
        }
        if (typeof s.wfmInvisibleOnStart === "boolean") {
          setWfmInvisibleOnStart(s.wfmInvisibleOnStart);
          wfmInvisibleOnStartRef.current = s.wfmInvisibleOnStart;
        }
        if (typeof s.wfmInvisibleOnClose === "boolean") {
          setWfmInvisibleOnClose(s.wfmInvisibleOnClose);
          wfmInvisibleOnCloseRef.current = s.wfmInvisibleOnClose;
        }
        if (typeof s.wfmAutoInvisible === "boolean") setWfmAutoInvisible(s.wfmAutoInvisible);
        if (typeof s.wfmAutoInvisibleMins === "number") setWfmAutoInvisibleMins(s.wfmAutoInvisibleMins);
        if (Array.isArray(s.arbitrationFavorites)) setArbFavorites(s.arbitrationFavorites.filter((x: unknown) => typeof x === "string"));
        if (typeof s.arbitrationLeadMins === "number") setArbLeadMins(clampLead(s.arbitrationLeadMins));
        const storedFilter = sanitizeTierKeys(s.arbitrationTierFilter);
        if (storedFilter) setArbTierFilter(storedFilter);
        const storedAlertTiers = sanitizeTierKeys(s.arbitrationAlertTiers);
        if (storedAlertTiers) setArbAlertTiers(storedAlertTiers);
        if (typeof s.arbitrationScheduleDays === "number") setArbScheduleDays(clampScheduleDays(s.arbitrationScheduleDays));
        if (typeof s.arbitrationOverlayEnabled === "boolean") {
          setArbOverlayEnabled(s.arbitrationOverlayEnabled);
          invoke("set_arbitration_overlay_enabled", { enabled: s.arbitrationOverlayEnabled });
        }
        if (Array.isArray(s.arbitrationAlertsFired)) arbFiredRef.current = s.arbitrationAlertsFired.filter((x: unknown) => typeof x === "string");
        setFilterPresets(parseFilterPresetSettings(s.filterPresets));
        return s;
      } catch {
        // Unblock saving even if the file failed to parse, since the backend
        // merges patches into the existing file.
        settingsLoadedRef.current = true;
        return null;
      }
    } catch {
      return null;
    }
  }, []);

  // ── Memory scanner toggle ───────────────────────────────────────────────────
  useEffect(() => {
    if (memoryScannerEnabled) {
      invoke("start_monitor").then(() => setMonitoring(true)).catch(() => {});
    } else {
      invoke("stop_monitor").then(() => setMonitoring(false)).catch(() => {});
    }
  }, [memoryScannerEnabled, setMonitoring]);

  // ── Blob log toggle ─────────────────────────────────────────────────────────
  useEffect(() => {
    invoke("set_blob_log", { enabled: blobLogEnabled }).catch(() => {});
  }, [blobLogEnabled]); // eslint-disable-line

  return {
    // Settings state
    memoryScannerEnabled,
    blobLogEnabled,
    autoDiagEnabled,
    overlayEnabled,
    overlayPriority,
    textScale,
    colorblindMode,
    clockFormat,
    foundryPageSize,
    relicPickEnabled,
    memTriggerEnabled,
    relicPickPriority,
    relicPickRefinement,
    relicPickLines,
    masteryExclude,
    wfmInvisibleOnStart,
    wfmInvisibleOnClose,
    wfmAutoInvisible,
    wfmAutoInvisibleMins,
    arbFavorites,
    arbLeadMins,
    arbTierFilter,
    arbAlertTiers,
    arbScheduleDays,
    arbOverlayEnabled,
    filterPresets,

    // Setters
    setMemoryScannerEnabled,
    setBlobLogEnabled,
    setAutoDiagEnabled,
    setOverlayEnabled,
    setOverlayPriority,
    setTextScale,
    setColorblindMode,
    setClockFormat,
    setFoundryPageSize,
    setRelicPickEnabled,
    setMemTriggerEnabled,
    setRelicPickPriority,
    setRelicPickRefinement,
    setRelicPickLines,
    setMasteryExclude,
    setWfmInvisibleOnStart,
    setWfmInvisibleOnClose,
    setWfmAutoInvisible,
    setWfmAutoInvisibleMins,
    setArbFavorites,
    setArbLeadMins,
    setArbTierFilter,
    setArbAlertTiers,
    setArbScheduleDays,
    setArbOverlayEnabled,
    setFilterPresets,

    // Refs
    settingsLoadedRef,
    settingsRef,
    wfmInvisibleOnStartRef,
    wfmInvisibleOnCloseRef,
    arbFiredRef,

    // Callbacks
    saveAllSettings,
    loadSettings,
  };
}
