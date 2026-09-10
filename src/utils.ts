/** Format a number with locale-appropriate separators (e.g. 1234 → "1,234"). */
export function fmt(n: number) { return n.toLocaleString(); }

/** CSS class for a positive or negative delta. */
export function deltaClass(d: number) { return d > 0 ? "delta-pos" : "delta-neg"; }

/** Render a signed delta string (e.g. +5 / -3). */
export function deltaText(d: number) { return d > 0 ? `+${fmt(d)}` : fmt(d); }

export function toggle<T>(arr: T[], val: T): T[] {
  return arr.includes(val) ? arr.filter(x => x !== val) : [...arr, val];
}

export function normalizeForWfm(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "");
}
