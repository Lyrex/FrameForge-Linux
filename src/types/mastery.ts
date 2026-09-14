export type MasteryState = "mastered" | "partial" | "missing" | "unknown";

/** Why no account can earn a source any more; settings exclude each class separately. */
export type Unobtainable = "founders" | "retiredEvent" | "removedNode";

export interface MasterySource {
  unique_name: string;
  name: string;
  category: string;
  image_name: string | null;
  mastery_req: number | null;
  cap: number;
  /** Absent while the source kind's progress is Unknown. */
  earned_rank: number | null;
  remaining_mastery: number | null;
  state: MasteryState;
  /** The corrections table's class, whatever the settings say. */
  unobtainable: Unobtainable | null;
  /** Settings exclude the class: the source sits in the Unobtainable bucket, outside `total`. */
  excluded: boolean;
}

export interface MasteryCounts {
  /** Excludes the `unobtainable` count. */
  total: number;
  mastered: number;
  partial: number;
  missing: number;
  unknown: number;
  unobtainable: number;
}

export interface MasteryCategory {
  category: string;
  counts: MasteryCounts;
  sources: MasterySource[];
}

export type ProvenanceState = "confirmed" | "unconfirmed" | "unknown";

export interface Provenance {
  state: ProvenanceState;
  /** Unix seconds; only Confirmed carries one. */
  observed_at: number | null;
}

export interface MasteryProvenance {
  equipment: Provenance;
  intrinsics: Provenance;
  nodes: Provenance;
  junctions: Provenance;
}

export type Stage = "level_claim" | "acquire";
export type Action = "level" | "claim" | "spend" | "buy";
export type Access = "available" | "blocked" | "unknown";

export interface VendorOffer {
  syndicate: string;
  tier: string;
  blueprint: boolean;
}

export interface TrackSpend {
  track: string;
  from: number;
  to: number;
}

/** The ranks that banked Intrinsic points buy today, cheapest first. */
export interface Spend {
  ranks: number;
  points: number;
  mastery: number;
  /** Lists only the tracks that gain a rank, in the game's order. */
  tracks: TrackSpend[];
}

export interface Opportunity extends MasterySource {
  stage: Stage;
  action: Action;
  owned: boolean;
  /** Null on caches from before levels were stored. */
  owned_level: number | null;
  build_completion_ms: number | null;
  vendors: VendorOffer[];
  spend: Spend | null;
  access: Access;
  blockers: string[];
}

export interface MasteryOverview {
  counts: MasteryCounts;
  categories: MasteryCategory[];
  provenance: MasteryProvenance;
  /** The backend sorts these by stage, then remaining mastery high to low with unknown last, then name. */
  opportunities: Opportunity[];
}
