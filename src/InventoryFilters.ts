import type { InventoryFilters } from "./types/filters";

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
