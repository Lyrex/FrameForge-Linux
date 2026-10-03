import type { ComponentPropsWithRef, ReactNode } from "react";

const CLOSE_BASE =
  "bg-transparent border-0 cursor-pointer text-muted text-[14px] px-[6px] py-[2px] shrink-0 rounded-[4px] transition-colors duration-100 hover:text-foreground hover:bg-[rgba(255,255,255,.06)]";

export function ModalCloseButton({ className = "", children, ...rest }: ComponentPropsWithRef<"button"> & { children?: ReactNode }) {
  return (
    <button className={`${CLOSE_BASE}${className ? ` ${className}` : ""}`} {...rest}>
      {children}
    </button>
  );
}
