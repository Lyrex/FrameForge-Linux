import type { ButtonHTMLAttributes, ReactNode } from "react";

const CAT_BASE =
  "flex items-center justify-between w-full px-[12px] py-[6px] bg-transparent border-0 border-l-2 border-l-transparent cursor-pointer text-left text-[13px] transition-[background,color] duration-100";
const CAT_ON = "text-accent! bg-[rgba(56,139,253,.08)]! border-l-accent!";
const CAT_OFF = "text-muted hover:bg-[rgba(255,255,255,.04)] hover:text-foreground";

export const CAT_LABEL = "flex-1";
export const CAT_COUNT =
  "flex items-center gap-[2px] text-[11px] bg-[rgba(56,139,253,.18)] text-accent py-[1px] px-[6px] rounded-[10px] min-w-[20px] text-center";
export const CAT_OWNED = "text-accent font-semibold";
export const CAT_SEP = "text-border";
export const CAT_TOTAL = "text-muted";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  label: ReactNode;
  labelClassName?: string;
  children?: ReactNode;
};

export function CategoryButton({ active = false, label, labelClassName = "", className = "", children, ...rest }: Props) {
  return (
    <button className={`${CAT_BASE} ${active ? CAT_ON : CAT_OFF}${className ? ` ${className}` : ""}`} {...rest}>
      <span className={`${CAT_LABEL}${labelClassName ? ` ${labelClassName}` : ""}`}>{label}</span>
      {children}
    </button>
  );
}
