import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
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
 *  cue to draw its placeholder. `key` changes on every attempt so an <img>
 *  keyed by it remounts and never keeps a broken-image icon. */
export function useImgLadder(urls: (string | undefined)[]): { src?: string; key: string; onError: () => void } {
  const baseUrl = useContext(ImgCacheDirContext);
  const key = urls.join("|");
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const srcs = useMemo(() => cdnCandidates(baseUrl, urls), [baseUrl, key]);
  const [idx, setIdx] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const retryTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    setIdx(0);
    setAttempt(0);
    return () => { if (retryTimer.current) clearTimeout(retryTimer.current); };
  }, [baseUrl, key]);
  const src = srcs[idx];
  const onError = () => {
    // A CDN miss is usually transient (rate limit, blip), so it gets two more
    // tries with backoff before the ladder moves on.
    if (src?.startsWith(CDN_PREFIX) && attempt < 2) {
      retryTimer.current = setTimeout(() => setAttempt(a => a + 1), 500 * (attempt + 1));
    } else {
      setIdx(i => i + 1);
      setAttempt(0);
    }
  };
  return { src, key: `${src}:${attempt}`, onError };
}
