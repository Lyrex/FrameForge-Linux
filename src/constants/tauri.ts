export const TAURI_EVENTS = {
  SETTINGS_UPDATED: "settings-updated",
  RELIC_TRIGGER: "relic-trigger",
  RELIC_REWARDS: "relic-rewards",
  RELIC_SCREEN: "relic-screen",
  RELIC_PICK_CLOSE: "relic-pick-close",
  RIVEN_MANUAL_CHECK: "riven-manual-check",
  RIVEN_WINDOW_READY: "riven-window-ready",
  RIVEN_ANALYSIS_UPDATE: "riven-analysis-update",
  RIVEN_ROLL_SAVED: "riven-roll-saved",
} as const;

export const TAURI_COMMANDS = {
  SAVE_SETTINGS: "save_settings",
  GET_CURRENT_QUANTITIES: "get_current_quantities",
  GET_RECIPES_BULK: "get_recipes_bulk",
  SAVE_RIVEN_ROLL: "save_riven_roll",
  MOVE_OVERLAY_OFFSCREEN: "move_overlay_offscreen",
  OPEN_URL: "plugin:opener|open_url",
} as const;
