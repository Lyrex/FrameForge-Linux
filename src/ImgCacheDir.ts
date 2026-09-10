import { createContext, useContext, useEffect, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

/** Absolute path of the img_cache folder in the user cache directory.
 *  Empty string = not yet known (images fall back to CDN). Set once on app startup. */
export const ImgCacheDirContext = createContext<string>("");

const CDN_PREFIX = "https://cdn.warframestat.us/img/";

export function cdnUrl(imageName?: string): string | undefined {
  return imageName ? CDN_PREFIX + imageName : undefined;
}

/** Put the locally cached copy of every CDN image in front of the CDN one. */
export function cdnCandidates(cacheDir: string, urls: (string | undefined)[]): string[] {
  const out: string[] = [];
  for (const url of urls) {
    if (!url) continue;
    if (cacheDir && url.startsWith(CDN_PREFIX)) out.push(convertFileSrc(`${cacheDir}/${url.slice(CDN_PREFIX.length)}`));
    out.push(url);
  }
  return [...new Set(out)];
}

/** Walk a list of image URLs, one step per load error, cache before CDN.
 *  `src` is undefined once every candidate has failed — that is the caller's
 *  cue to draw its placeholder. */
export function useImgLadder(urls: (string | undefined)[]): { src?: string; onError: () => void } {
  const baseUrl = useContext(ImgCacheDirContext);
  const key = urls.join("|");
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const srcs = useMemo(() => cdnCandidates(baseUrl, urls), [baseUrl, key]);
  const [idx, setIdx] = useState(0);
  useEffect(() => setIdx(0), [baseUrl, key]);
  return { src: srcs[idx], onError: () => setIdx(i => i + 1) };
}
