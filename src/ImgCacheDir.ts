import { createContext, useCallback, useRef, useState } from "react";

/** Absolute path to the local image cache directory (%LOCALAPPDATA%\warframe-companion\img_cache).
 *  Empty string = not yet known (images fall back to CDN). Set once on app startup. */
export const ImgCacheDirContext = createContext<string>("");

const CDN_PREFIX = "https://cdn.warframestat.us/img/";

/** Build the CDN URL for an image name (e.g. "ash-prime.png" → CDN URL). */
export function cdnUrl(imageName?: string): string | undefined {
  if (!imageName) return undefined;
  return `${CDN_PREFIX}${imageName}`;
}

/**
 * Given a primary base URL and a list of fallback candidates (undefined entries
 * are skipped), return the deduplicated list of URLs to try in order.
 */
export function cdnCandidates(
  baseUrl: string,
  urls: (string | undefined)[]
): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const u of [baseUrl, ...urls]) {
    if (u && !seen.has(u)) {
      seen.add(u);
      result.push(u);
    }
  }
  return result;
}

/**
 * Walk a list of image URLs in priority order: try each in sequence on error.
 * Returns { src, onError } to spread onto an <img>.
 */
export function useImgLadder(urls: (string | undefined)[]): {
  src: string | undefined;
  onError: () => void;
} {
  const candidates = urls.filter(Boolean) as string[];
  const indexRef = useRef(0);
  const [src, setSrc] = useState<string | undefined>(candidates[0]);

  const onError = useCallback(() => {
    indexRef.current += 1;
    if (indexRef.current < candidates.length) {
      setSrc(candidates[indexRef.current]);
    } else {
      setSrc(undefined);
    }
  }, [candidates.join(",")]); // eslint-disable-line react-hooks/exhaustive-deps

  return { src, onError };
}
