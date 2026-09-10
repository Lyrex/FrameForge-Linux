import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { cdnUrl, useImgLadder } from "./ImgCacheDir";

function BlueprintIcon() {
  return (
    <svg className="img-fallback" viewBox="0 0 32 32" fill="none" xmlns="http://www.w3.org/2000/svg">
      <rect x="5" y="2" width="17" height="22" rx="1.5" fill="#0d1f33" stroke="#388bfd" strokeWidth="1.2"/>
      <path d="M18 2 L22 6 L18 6 Z" fill="#388bfd" opacity="0.5"/>
      <line x1="8" y1="11" x2="19" y2="11" stroke="#388bfd" strokeWidth="1" opacity="0.9"/>
      <line x1="8" y1="14" x2="19" y2="14" stroke="#388bfd" strokeWidth="1" opacity="0.9"/>
      <line x1="8" y1="17" x2="14" y2="17" stroke="#388bfd" strokeWidth="1" opacity="0.9"/>
      <circle cx="23" cy="23" r="6" fill="#0d1117" stroke="#388bfd" strokeWidth="1.2"/>
      <line x1="23" y1="20" x2="23" y2="26" stroke="#388bfd" strokeWidth="1.2"/>
      <line x1="20" y1="23" x2="26" y2="23" stroke="#388bfd" strokeWidth="1.2"/>
    </svg>
  );
}

interface Props {
  imageName?: string | (string | undefined)[];
  category?: string;
  size?: number;
  className?: string;
  style?: CSSProperties;
  fallback?: ReactNode;
}

const toUrl = (n?: string) => n?.startsWith("http") || n?.startsWith("/") ? n : cdnUrl(n);

export default function ItemImg({ imageName, category = "?", size, className = "img", style, fallback }: Props) {
  const { src, onError } = useImgLadder((Array.isArray(imageName) ? imageName : [imageName]).map(toUrl));
  const ref = useRef<HTMLImageElement>(null);
  const box = size === undefined ? style : { width: size, height: size, flexShrink: 0 as const, ...style };

  useEffect(() => {
    if (ref.current?.complete) ref.current.classList.add("img-loaded");
  }, [src]);

  if (!src) {
    if (fallback !== undefined) return fallback;
    if (category === "Blueprints") return <BlueprintIcon />;
    return <span className="img-fallback" style={{ ...box, fontSize: size && size * 0.35 }}>{category[0].toUpperCase()}</span>;
  }
  // key remounts the element per candidate so a broken-image icon never lingers.
  return (
    <img key={src} ref={ref} className={className} style={box} src={src} alt="" loading="lazy"
      onError={onError} onLoad={() => ref.current?.classList.add("img-loaded")} />
  );
}
