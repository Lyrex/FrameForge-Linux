import type { TierKey } from "../arbitration/arbitrationTiers";
import type { ClockFormat } from "../lib/clockFormat";
export type RelicOverlayPriority = "completion" | "plat" | "ducat" | "setPlat";
export type RelicPickPriority = "unowned" | "ducat" | "platinum";
export type RelicRefinement = "intact" | "exceptional" | "flawless" | "radiant";
export type RelicPickLines = "all" | "best" | "estimated";
export type FoundryPageSize = 30 | 60 | 100;

export type FissureVariant = "normal" | "hard" | "storm";

export interface FissureWatch {
  id: string;
  tier: string;
  missionType: string;
  variant: "any" | FissureVariant;
}

export interface SettingsSnapshot {
  overlayEnabled: boolean; overlayPriority: RelicOverlayPriority; textScale: number; colorblindMode: boolean;
  clockFormat: ClockFormat; memoryScannerEnabled: boolean;
  blobLogEnabled: boolean; autoDiagEnabled: boolean; tracked: string[];
  arbitrationFavorites: string[]; arbitrationLeadMins: number; arbitrationOverlayEnabled: boolean;
  arbitrationTierFilter: TierKey[]; arbitrationAlertTiers: TierKey[]; arbitrationScheduleDays: number;
  favorites: string[]; timerFavorites: string[]; fissureWatches: FissureWatch[]; fissureNotifications: boolean;
  modularWidth: number; modularSectionOrder: string[]; modularPopout: boolean; wfmInvisibleOnStart: boolean;
  wfmInvisibleOnClose: boolean; wfmAutoInvisible: boolean; wfmAutoInvisibleMins: number; relicPickEnabled: boolean;
  relicPickPriority: RelicPickPriority; relicPickRefinement: RelicRefinement;
  relicPickLines: RelicPickLines; foundryPageSize: FoundryPageSize; memTriggerEnabled: boolean;
}
