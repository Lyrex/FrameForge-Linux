import { forwardRef, type ButtonHTMLAttributes, type HTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";

const CHIP_BASE = "cursor-pointer whitespace-nowrap rounded-[12px] border border-border bg-transparent px-[9px] py-[2px] text-[11px] text-muted transition-all duration-[120ms]";
const CHIP_HOVER = "hover:border-[rgba(255,255,255,.25)]! hover:text-foreground!";
const CHIP_ACTIVE = "border-accent! bg-[rgba(56,139,253,.1)]! text-accent!";
const CHIP_RESET = "border-[rgba(255,255,255,.2)]! text-foreground! hover:border-[rgba(255,255,255,.4)]! disabled:border-border! disabled:text-muted! disabled:hover:border-border! disabled:hover:text-muted! disabled:opacity-[.55] disabled:cursor-default";

type FilterChipProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  reset?: boolean;
  children?: ReactNode;
};

export const FilterChip = forwardRef<HTMLButtonElement, FilterChipProps>(function FilterChip({ active = false, reset = false, className = "", children, ...rest }, ref) {
  return <button ref={ref} className={`${CHIP_BASE} ${reset ? CHIP_RESET : CHIP_HOVER}${active ? ` ${CHIP_ACTIVE}` : ""}${className ? ` ${className}` : ""}`} {...rest}>{children}</button>;
});

export function FilterBar({ className = "", ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={`flex shrink-0 flex-wrap items-center gap-1 border-b border-border bg-background px-3 py-1${className ? ` ${className}` : ""}`} {...rest} />;
}

export function FilterSeparator({ className = "", ...rest }: HTMLAttributes<HTMLSpanElement>) {
  return <span className={`mx-[2px] h-[14px] w-px shrink-0 bg-border${className ? ` ${className}` : ""}`} {...rest} />;
}

export function FilterLabel({ className = "", ...rest }: HTMLAttributes<HTMLSpanElement>) {
  return <span className={`whitespace-nowrap text-[11px] text-muted${className ? ` ${className}` : ""}`} {...rest} />;
}

export const FoundrySearch = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(function FoundrySearch({ className = "", ...rest }, ref) {
  return <input ref={ref} className={`box-border w-full rounded-[6px] border border-border bg-surface px-2 py-1 text-[12px] text-foreground outline-none focus:border-accent${className ? ` ${className}` : ""}`} {...rest} />;
});

export function EmptyMessage({ className = "", ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={`px-6 py-10 text-center leading-[1.6] text-muted${className ? ` ${className}` : ""}`} {...rest} />;
}
