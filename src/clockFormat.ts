/// The app-wide clock format setting. "auto" follows the system locale's own
/// hour cycle; the explicit choices force one side or the other.
export type ClockFormat = "auto" | "12h" | "24h";

/// Intl options for the format; undefined hour12 means the locale decides.
export function clockOptions(format: ClockFormat) {
  const opts: Intl.DateTimeFormatOptions = { hour: "2-digit", minute: "2-digit" };
  if (format === "12h") opts.hour12 = true;
  else if (format === "24h") opts.hour12 = false;
  return opts;
}

/// `unix` is in seconds, as the app's event timestamps are.
export function fmtClock(unix: number, format: ClockFormat = "auto", locale = "en-US") {
  return new Date(unix * 1000).toLocaleTimeString(locale, clockOptions(format));
}