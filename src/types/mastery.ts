export type MasteryState = "mastered" | "partial" | "missing" | "unknown";

/** Why no account can earn a source any more; settings exclude each class separately. */
export type Unobtainable = "founders" | "retiredEvent" | "removedNode";

export type Mode = "normal" | "steel_path";

/** A node's normal and Steel Path clears are two sources. The Steel Path row's `unique_name` ends in `/steel_path`. */
export interface NodeInfo {
  key: string;
  planet: string;
  mode: Mode;
  junction: boolean;
}

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
  /** Present on star chart rows only, where `cap` is 1 and `earned_rank` is 0 or 1. */
  node?: NodeInfo;
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

export type Stage = "level_claim" | "craft" | "acquire";
/** Craft means everything is in stock. Build means intermediates need crafting first. Farm means parts are short and no vendor sells them. Trade is a whole item only players sell, so it belongs to the platinum view alone. */
export type Action = "level" | "claim" | "spend" | "craft" | "build" | "buy" | "farm" | "trade" | "complete" | "unlock";
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

export interface Requirement {
  unique_name: string;
  name: string;
  needed: number;
  from_stock: number;
  short: number;
}

export interface Build {
  unique_name: string;
  name: string;
  crafts: number;
}

export interface CraftPlan {
  requirements: Requirement[];
  builds: Build[];
  /** Null as soon as any recipe in the plan carries no price. */
  credits: number | null;
  credits_short: number;
}

/** One owned relic, at one refinement, that drops the part. */
export interface RelicStock {
  name: string;
  count: number;
  /** The reward's share of one roll after the refinement's table is normalized to one. Null when the table carries no chances. */
  chance: number | null;
}

export interface RelicPart {
  unique_name: string;
  name: string;
  needed: number;
  /** Sorted with the best chance first. */
  relics: RelicStock[];
}

/**
 * Complete: every part has relics enough to roll, and the chance that they all drop.
 * Partial: `missing` names parts no owned relic drops, and `short` names parts the owned relics cannot yield together.
 * Unknown: a relevant relic's table carries no chances, or the walk would take more states than allowed.
 */
export type Coverage =
  | { kind: "complete"; probability: number }
  | { kind: "partial"; missing: string[]; short: string[] }
  | { kind: "unknown" };

/** The chance covers the relic parts only, and the other shortages stay in `craft`. */
export interface RelicRoute {
  parts: RelicPart[];
  coverage: Coverage;
}

/**
 * A warframe.market listing. A price with a fetch time is a quote of that age; a price without one
 * came from a cache written before quotes were stamped; no price with a fetch time means the market
 * does not list the slug; neither means nobody has asked yet.
 */
export interface Listing {
  slug: string;
  name: string;
  price: number | null;
  /** Unix seconds. */
  fetched_at: number | null;
}

export interface PartListing extends Listing {
  unique_name: string;
  /** The whole recipe takes this many, owned or not. */
  needed: number;
  /** The finish is still short this many after projected stock. */
  short: number;
}

export type Route = "parts" | "set";

export interface Cost {
  platinum: number;
  route: Route;
}

/** What platinum buys toward one source. Each total is null while any part it sums lacks a price. */
export interface Purchase {
  parts: PartListing[];
  /** Holds the complete set, or the item itself where nothing is crafted. */
  set: Listing | null;
  missing_total: number | null;
  full_total: number | null;
  /** The cheaper of the missing parts and the set. A tie goes to the set. */
  cheapest_finish: Cost | null;
  full_purchase: Cost | null;
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
  /** A source that is neither owned nor building carries its plan whenever a recipe exists. */
  craft: CraftPlan | null;
  /** Present when something short in the plan drops from a relic. */
  relic: RelicRoute | null;
  purchase: Purchase | null;
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
