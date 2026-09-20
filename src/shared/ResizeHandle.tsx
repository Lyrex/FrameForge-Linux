import { useEffect, useRef } from "react";

interface Props {
  className: string;
  value: number;
  axis: "x" | "y";
  direction: 1 | -1;
  clamp: (value: number) => number;
  onValueChange: (value: number) => void;
  onValueCommit?: (value: number) => void;
}

export function ResizeHandle({ className, value, axis, direction, clamp, onValueChange, onValueCommit }: Props) {
  const frameRef = useRef<number | null>(null);
  const valueRef = useRef(value);
  const cleanupRef = useRef<(() => void) | null>(null);

  useEffect(() => () => cleanupRef.current?.(), []);

  return (
    <div
      className={className}
      onMouseDown={event => {
        event.preventDefault();
        event.stopPropagation();
        cleanupRef.current?.();
        valueRef.current = value;
        const startCoordinate = axis === "x" ? event.clientX : event.clientY;
        const startValue = value;
        const scale = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--ff-scale")) || 1;
        document.body.style.userSelect = "none";

        const onMove = (moveEvent: MouseEvent) => {
          const coordinate = axis === "x" ? moveEvent.clientX : moveEvent.clientY;
          valueRef.current = clamp(startValue + direction * (coordinate - startCoordinate) / scale);
          if (frameRef.current === null) {
            frameRef.current = window.requestAnimationFrame(() => {
              onValueChange(valueRef.current);
              frameRef.current = null;
            });
          }
        };
        const onUp = () => {
          onValueChange(valueRef.current);
          onValueCommit?.(valueRef.current);
          cleanup();
        };
        const cleanup = () => {
          window.removeEventListener("mousemove", onMove);
          window.removeEventListener("mouseup", onUp);
          window.removeEventListener("blur", onUp);
          if (frameRef.current !== null) {
            window.cancelAnimationFrame(frameRef.current);
            frameRef.current = null;
          }
          document.body.style.userSelect = "";
          cleanupRef.current = null;
        };
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
        window.addEventListener("blur", onUp);
        cleanupRef.current = cleanup;
      }}
    />
  );
}
