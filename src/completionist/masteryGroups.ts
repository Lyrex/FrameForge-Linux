import type { MasterySource } from "../types/mastery";

const GROUP_ORDER = ["Standard", "Zaw", "Kitgun", "Amp", "Prime", "Kuva", "Tenet", "Coda", "Wraith", "Vandal", "Prisma", "MK1"];

export function masteryGroup(source: MasterySource): string {
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
  return [...byGroup.entries()]
    .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
    .map(([group, list]) => ({ group, sources: list.sort((a, b) => a.name.localeCompare(b.name)) }));
}
