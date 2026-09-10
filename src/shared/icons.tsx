import type { CSSProperties } from "react";

const iconStyle: CSSProperties = { objectFit: "contain", flexShrink: 0, verticalAlign: "middle" };

export function PlatIcon({ size = 14, style }: { size?: number; style?: CSSProperties }) {
  return <img src="/platinum.webp" alt="plat" width={size} height={size} style={{ ...iconStyle, ...style }} />;
}
export function DucatIcon({ size = 14, style }: { size?: number; style?: CSSProperties }) {
  return <img src="/ducats.webp" alt="ducat" width={size} height={size} style={{ ...iconStyle, ...style }} />;
}
