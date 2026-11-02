import { TriangleAlert, Undo2 } from "lucide-react";
import type { ReactNode } from "react";

/** `pending` is an undo window that closes on its own; `danger` is a durable failure notice. */
type ToastTone = "pending" | "danger";

interface ToastProps {
  tone?: ToastTone;
  children: ReactNode;
  actionLabel: string;
  onAction: () => void;
  /** Show the closing undo window as a hairline along the bottom edge. */
  countdown?: boolean;
}

export function Toast({ tone = "pending", children, actionLabel, onAction, countdown = true }: ToastProps) {
  const danger = tone === "danger";
  return (
    <div className={danger ? "toast toast--danger" : "toast"} role={danger ? "alert" : "status"}>
      <span className="toast__glyph" aria-hidden="true">
        {danger ? <TriangleAlert size={14} strokeWidth={1.6} /> : <Undo2 size={14} strokeWidth={1.6} />}
      </span>
      <span className="toast__message">{children}</span>
      <button type="button" className="toast__action" onClick={onAction}>{actionLabel}</button>
      {countdown && !danger ? <span className="toast__countdown" aria-hidden="true" /> : null}
    </div>
  );
}
