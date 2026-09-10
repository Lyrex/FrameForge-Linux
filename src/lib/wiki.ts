import { openUrl } from "@tauri-apps/plugin-opener";
import { WARFRAME_WIKI_BASE } from "../constants/urls";

export function wikiUrl(name: string) {
  return `${WARFRAME_WIKI_BASE}/Special:Search?search=${encodeURIComponent(name)}`;
}

export function openWiki(name: string) {
  openUrl(wikiUrl(name));
}

export function copyWikiLink(name: string) {
  return navigator.clipboard.writeText(wikiUrl(name));
}
