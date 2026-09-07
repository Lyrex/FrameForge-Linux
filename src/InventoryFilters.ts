export type InventorySortMode = "qty-desc" | "qty-asc" | "name-asc" | "name-desc" | "recent";

export interface InventoryFilters {
  category: string;
  search: string;
  filterOwned: boolean;
  filterRecent: boolean;
  filterPrime: boolean;
  filterVaulted: boolean;
  filterUnvaulted: boolean;
  filterRank: number | "unranked" | null;
  sortMode: InventorySortMode;
}

export const INVENTORY_FILTERS_DEFAULT: InventoryFilters = {
  category: "all",
  search: "",
  filterOwned: false,
  filterRecent: false,
  filterPrime: false,
  filterVaulted: false,
  filterUnvaulted: false,
  filterRank: null,
  sortMode: "qty-desc",
};
