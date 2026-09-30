// Debug builds of the backend point at `.frameforge-dev/` instead of the
// installed app's directories, so the product name says so everywhere.
export const IS_DEV = import.meta.env.DEV;

export const APP_TITLE = IS_DEV ? "FrameForge Dev" : "FrameForge";
