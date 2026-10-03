import type { ComponentPropsWithRef, ReactNode } from "react";

export type ButtonVariant = "secondary" | "danger";

const BTN_BASE =
  "px-[14px] py-[6px] rounded-[6px] text-[12px] cursor-pointer transition-all duration-150 shrink-0 whitespace-nowrap";

const VARIANT_CLASS: Record<ButtonVariant, string> = {
  secondary:
    "border border-border bg-surface text-foreground hover:enabled:border-accent hover:enabled:text-accent disabled:opacity-50 disabled:cursor-default",
  danger:
    "border border-[rgba(248,81,73,.4)] bg-[rgba(248,81,73,.08)] text-danger hover:border-danger hover:bg-[rgba(248,81,73,.15)]",
};

type Props = ComponentPropsWithRef<"button"> & {
  variant?: ButtonVariant;
  children?: ReactNode;
};

export function ActionButton({ variant = "secondary", className = "", children, ...rest }: Props) {
  return (
    <button className={`${BTN_BASE} ${VARIANT_CLASS[variant]}${className ? ` ${className}` : ""}`} {...rest}>
      {children}
    </button>
  );
}

export function SecondaryButton(props: Omit<Props, "variant">) {
  return <ActionButton {...props} />;
}

export function DangerButton(props: Omit<Props, "variant">) {
  return <ActionButton variant="danger" {...props} />;
}
