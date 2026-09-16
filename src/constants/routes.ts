import type { RouteKind } from "../types/mastery";

export function routeText(route: RouteKind): string {
  switch (route.kind) {
    case "craft": return "Recipe";
    case "relic": return "Relic drop";
    case "drop": return "Mission drop";
    case "vendor": return "Vendor";
    case "trade": return "Player trade";
    case "adversary": return "Lich, Sister or Technocyte Coda reward";
    case "conservation": return "Revived by Son on Deimos";
    case "market_credits": return `Market, ${route.credits.toLocaleString("en-US")} credits blueprint`;
    case "baro": return "Baro Ki'Teer";
    case "nightwave": return "Nightwave";
    case "quest": return "Quest";
    case "research": return `Research at ${route.lab}`;
  }
}

/** The kinds the row does not already tell through its action, relic pill or drop list. */
const ORIGIN_KINDS: ReadonlySet<RouteKind["kind"]> = new Set(["adversary", "conservation", "market_credits", "baro", "nightwave", "quest", "research"]);

export function isOrigin(route: RouteKind): boolean {
  return ORIGIN_KINDS.has(route.kind);
}

export const SOURCE_UNKNOWN = "Source unknown";
