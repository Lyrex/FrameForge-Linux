// Shapes returned by the riven grading and auction-search commands.
//
// Grading happens in the backend so the owned list, the auction results and the
// Market Helper badge all judge a roll on one scale. Nothing here is computed in
// the frontend: the backend already resolved the weapon, ran the stat formula and
// scored the roll, and these types are what it hands over.

/** Which stage of unlocking a riven is at. Mirrors `RivenState` in the scanner. */
export type RivenState = "unrevealed" | "revealed" | "unlocked";

/** How a stat's value should be read: percentage, multiplier, metres, seconds, or a flat count. */
export type StatUnit = "%" | "x" | "m" | "s" | "n";

/** One rolled stat, placed in the range it could have rolled in at this rank. */
export interface GradedStat {
  /** The game's own stat tag, e.g. `WeaponCritChanceMod`. */
  tag: string;
  /**
   * The roll as the game stores it. Posting an auction needs the untouched
   * value, since the market recomputes the stat from it.
   */
  raw: number;
  /** Display label for the tag, e.g. "Critical Chance". */
  label: string;
  /** Preformatted value with sign and unit, e.g. "+153.7%". */
  display: string;
  /**
   * The analyzer's name for this stat, or null when no analyzer stat corresponds
   * to the tag — an unmapped stat takes no part in the score.
   */
  analyzer_name: string | null;
  value: number;
  /** Weakest and strongest this stat could have rolled at this rank. */
  min: number;
  max: number;
  /** `min` and `max` formatted like `display`, so a range reads in the roll's units. */
  min_display: string;
  max_display: string;
  /** Where `value` sits between `min` and `max`, 0 to 1. */
  position: number;
  unit: StatUnit;
  positive: boolean;
}

/** One alternative build from the wanted-stats database, scored against the roll. */
export interface AlternativeResult {
  label: string;
  matched: string[];
  missing: string[];
  score: number;
  verdict: string;
}

/** The analyzer's judgement of a roll. Absent when the weapon has no wanted-stats entry. */
export interface RivenAnalysis {
  weapon: string;
  matched_positives: string[];
  missing_positives: string[];
  safe_negatives_present: string[];
  harmful_negatives: string[];
  total_wanted: number;
  score: number;
  verdict: string;
  notes: string;
  alternatives: AlternativeResult[];
}

/**
 * A riven the scanner holds, with its stats placed in range and its roll graded.
 *
 * `analysis` is null whenever the roll cannot be judged — an unrevealed or
 * revealed-but-locked riven has no stats yet, and an unlocked riven whose weapon
 * the wanted-stats database does not cover has no scale to be judged against.
 * A null analysis means "no grading data", never a score of zero.
 */
export interface GradedRiven {
  item_id: string;
  item_type: string;
  state: RivenState;
  /** Weapon `unique_name`. Only present once the riven is unlocked. */
  compat: string | null;
  /** Weapon display name, resolved from `compat`. */
  weapon_name: string | null;
  /** Rifle, Shotgun, Pistol, Melee, Zaw, Arch-gun, or Riven when unknown. */
  category: string;
  polarity: string | null;
  /** Mastery rank required to equip. */
  mastery_level: number | null;
  mod_rank: number;
  rerolls: number;
  /** Above 1 for a stack of identical unrevealed rivens. */
  count: number;
  mod_name: string;
  /** The weapon's disposition, as used to compute the stat ranges. */
  disposition: number;
  buffs: GradedStat[];
  curses: GradedStat[];
  analysis: RivenAnalysis | null;
  /** Challenge to unveil the riven. Only present while it is revealed but locked. */
  challenge: string | null;
  complication: string | null;
}

// ── Auction search ────────────────────────────────────────────────────────────

/** What the user is looking for on the market. Every field beyond the weapon narrows the search. */
export interface AuctionQuery {
  /** Weapon display name. Empty searches every weapon. */
  weapon: string;
  /** Analyzer stat names the buyer wants as positives. */
  positive_stats: string[];
  /** Analyzer stat name the buyer will accept as the negative. */
  negative_stat: string | null;
  polarity: string | null;
  mastery_min: number | null;
  mastery_max: number | null;
  rerolls_min: number | null;
  rerolls_max: number | null;
  buyout_only: boolean;
}

/** One auction, normalized to the same riven shape as the owned list and graded the same way. */
export interface GradedAuction {
  auction_id: string;
  url: string;
  starting_price: number | null;
  buyout_price: number | null;
  top_bid: number | null;
  seller_name: string;
  /** ingame, online, or offline. */
  seller_status: string;
  seller_reputation: number;
  riven: GradedRiven;
}
