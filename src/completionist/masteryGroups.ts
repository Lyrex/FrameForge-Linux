import type { MasterySource } from "../types/mastery";

const GROUP_ORDER = ["Standard", "Zaw", "Kitgun", "Amp", "Prime", "Kuva", "Tenet", "Coda", "Wraith", "Vandal", "Prisma", "MK1"];

export function masteryGroup(source: MasterySource): string {
  if (source.category === "Intrinsics") return "Intrinsics";
  if (source.node) return source.node.planet;
  const path = source.unique_name;
  if (path.includes("/Ostron/Melee/")) return "Zaw";
  if (path.includes("/SolarisUnited/") || path.includes("/InfKitGun/")) return "Kitgun";
  if (path.includes("/OperatorAmplifiers/")) return "Amp";
  const name = source.name;
  if (/^mk1-/i.test(name)) return "MK1";
  if (name.startsWith("Kuva ")) return "Kuva";
  if (name.startsWith("Tenet ")) return "Tenet";
  if (name.startsWith("Coda ")) return "Coda";
  if (name.includes("Prime")) return "Prime";
  if (name.includes("Wraith")) return "Wraith";
  if (name.includes("Vandal")) return "Vandal";
  if (name.includes("Prisma")) return "Prisma";
  return "Standard";
}

export interface SourceGroup {
  group: string;
  sources: MasterySource[];
}

export function groupSources(sources: MasterySource[]): SourceGroup[] {
  const byGroup = new Map<string, MasterySource[]>();
  for (const source of sources) {
    const group = masteryGroup(source);
    const list = byGroup.get(group) ?? [];
    list.push(source);
    byGroup.set(group, list);
  }
  const rank = (group: string) => {
    const i = GROUP_ORDER.indexOf(group);
    return i === -1 ? GROUP_ORDER.length : i;
  };
  // Planets are not in GROUP_ORDER, so they rank equal and keep the chart
  // order they arrived in. Their rows stay unsorted for the same reason: the
  // backend already lists junctions first and pairs each node's modes.
  return [...byGroup.entries()]
    .sort(([a], [b]) => rank(a) - rank(b))
    .map(([group, list]) => ({ group, sources: list[0]?.node ? list : list.sort((a, b) => a.name.localeCompare(b.name)) }));
}
