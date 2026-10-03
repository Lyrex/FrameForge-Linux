export const CONN_CHIP =
  "inline-flex items-center gap-[5px] text-[11px] px-[8px] py-[3px] rounded-[4px] border bg-[rgba(255,255,255,.03)] text-muted whitespace-nowrap select-none";
export const CONN_BUTTON =
  "appearance-none [font:inherit] cursor-pointer text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2";
export const CONN_DOT = "w-[6px] h-[6px] rounded-full shrink-0";
export const CONN_LABEL = "font-medium text-foreground opacity-70";

export type ConnState = "online" | "warn" | "offline" | "disabled" | "overlay";

export const CONN_STATUS: Record<ConnState, { chip: string; dot: string; detail: string }> = {
  online: {
    chip: "border-[rgba(63,185,80,.25)]",
    dot: "bg-[#3fb950] shadow-[0_0_5px_#3fb95066]",
    detail: "text-[#3fb950]",
  },
  warn: {
    chip: "border-[rgba(210,153,34,.3)]",
    dot: "bg-[#d29922]",
    detail: "text-[#d29922]",
  },
  offline: {
    chip: "border-border",
    dot: "bg-[#6e7681]",
    detail: "text-muted",
  },
  disabled: {
    chip: "border-border opacity-45",
    dot: "bg-[#6e7681]",
    detail: "text-muted",
  },
  overlay: {
    chip: "border-[rgba(100,160,220,.25)]",
    dot: "bg-[#9ecaed] animate-[pulse-ocr_1.2s_ease-in-out_infinite]",
    detail: "text-[#9ecaed] font-mono",
  },
};
