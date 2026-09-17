import { useRef, type KeyboardEvent, type PointerEvent, type RefObject } from "react";

interface ResizeHandleProps {
  axis: "x" | "y";
  className: string;
  cssVariable: `--${string}`;
  defaultValue: number;
  direction: 1 | -1;
  label: string;
  max: number;
  min: number;
  onCommit: (value: number) => void;
  targetRef: RefObject<HTMLElement | null>;
  value: number;
}

interface DragState {
  pointerId: number;
  startPosition: number;
  startValue: number;
}

function clamp(value: number, min: number, max: number) {
  return Math.round(Math.min(max, Math.max(min, value)));
}

export function ResizeHandle({
  axis,
  className,
  cssVariable,
  defaultValue,
  direction,
  label,
  max,
  min,
  onCommit,
  targetRef,
  value,
}: ResizeHandleProps) {
  const dragRef = useRef<DragState | undefined>(undefined);
  const pendingValueRef = useRef(value);

  const apply = (nextValue: number) => {
    const next = clamp(nextValue, min, max);
    pendingValueRef.current = next;
    targetRef.current?.style.setProperty(cssVariable, `${next}px`);
    return next;
  };

  const finishDrag = (event: PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = undefined;
    event.currentTarget.classList.remove("is-dragging");
    onCommit(pendingValueRef.current);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const decreaseKey = axis === "x" ? "ArrowLeft" : "ArrowUp";
    const increaseKey = axis === "x" ? "ArrowRight" : "ArrowDown";
    let next: number | undefined;
    if (event.key === decreaseKey) next = value - 8 * direction;
    if (event.key === increaseKey) next = value + 8 * direction;
    if (event.key === "Home") next = min;
    if (event.key === "End") next = max;
    if (next === undefined) return;
    event.preventDefault();
    onCommit(apply(next));
  };

  return (
    <div
      aria-label={label}
      aria-orientation={axis === "x" ? "vertical" : "horizontal"}
      aria-valuemax={max}
      aria-valuemin={min}
      aria-valuenow={value}
      className={`resize-handle ${className}`}
      onDoubleClick={() => onCommit(apply(defaultValue))}
      onKeyDown={handleKeyDown}
      onLostPointerCapture={finishDrag}
      onPointerCancel={finishDrag}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        pendingValueRef.current = value;
        dragRef.current = {
          pointerId: event.pointerId,
          startPosition: axis === "x" ? event.clientX : event.clientY,
          startValue: value,
        };
        event.currentTarget.classList.add("is-dragging");
        event.currentTarget.setPointerCapture(event.pointerId);
        event.preventDefault();
      }}
      onPointerMove={(event) => {
        const drag = dragRef.current;
        if (!drag || drag.pointerId !== event.pointerId) return;
        const position = axis === "x" ? event.clientX : event.clientY;
        apply(drag.startValue + (position - drag.startPosition) * direction);
      }}
      onPointerUp={finishDrag}
      role="separator"
      tabIndex={0}
      title={label}
    />
  );
}
