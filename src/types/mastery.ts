export type MasteryState = "mastered" | "partial" | "missing" | "unknown";

export interface MasterySource {
  unique_name: string;
  name: string;
  category: string;
  image_name: string | null;
  mastery_req: number | null;
  cap: number;
  /** Absent while equipment progress is Unknown. */
  earned_rank: number | null;
  state: MasteryState;
}

export interface MasteryCounts {
  total: number;
  mastered: number;
  partial: number;
  missing: number;
  unknown: number;
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

export interface MasteryOverview {
  counts: MasteryCounts;
  categories: MasteryCategory[];
  provenance: MasteryProvenance;
}
