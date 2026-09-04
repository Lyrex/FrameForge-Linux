function read(key: string, fallback: number): number {
  const v = parseFloat(localStorage.getItem(key) ?? "");
  return Number.isFinite(v) && v > 0 ? v : fallback;
}

export const appScale = () => read("ff-text-scale", 1);

// Overlays read their own key, so they can scale independently of the app
// window. Nothing writes that key yet, so for now they follow the app text size.
export const overlayScale = () => read("ff-overlay-scale", appScale());

export function applyScale(isOverlay: boolean) {
  const s = isOverlay ? overlayScale() : appScale();
  document.documentElement.style.setProperty("--ff-scale", s.toString());
}
