import * as Primitive from "@radix-ui/react-dialog";
import { useId, useRef, type ReactNode, type RefObject } from "react";
import styles from "./Dialog.module.css";

interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  initialFocusRef?: RefObject<HTMLElement | null>;
  returnFocusRef?: RefObject<HTMLElement | null>;
  dismissOnOutsideClick: boolean;
  role?: "dialog" | "alertdialog";
  className?: string;
}

/** Controlled dialogs can open from commands without a Radix Trigger. */
export function Dialog({ open, onOpenChange, title, description, children,
  initialFocusRef, returnFocusRef, dismissOnOutsideClick, role = "dialog", className }: DialogProps) {
  const previousFocus = useRef<HTMLElement | null>(null);
  const descriptionId = useId();
  return <Primitive.Root open={open} onOpenChange={onOpenChange}>
    <Primitive.Portal>
      <Primitive.Overlay className={styles.overlay}>
        <Primitive.Content role={role}
          className={[styles.content, className].filter(Boolean).join(" ")}
          aria-describedby={description ? descriptionId : undefined}
          onOpenAutoFocus={event => {
            previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
            if (initialFocusRef?.current) {
              event.preventDefault();
              initialFocusRef.current.focus();
            }
          }}
          onCloseAutoFocus={event => {
            event.preventDefault();
            const target = returnFocusRef?.current ?? previousFocus.current;
            if (target?.isConnected) target.focus({ preventScroll: true });
          }}
          onPointerDownOutside={event => { if (!dismissOnOutsideClick) event.preventDefault(); }}
          onEscapeKeyDown={event => event.stopPropagation()}
          onKeyDown={event => event.stopPropagation()}>
          <Primitive.Title className={styles.title}>{title}</Primitive.Title>
          {description ? <Primitive.Description id={descriptionId} className={styles.description}>{description}</Primitive.Description> : null}
          {children}
        </Primitive.Content>
      </Primitive.Overlay>
    </Primitive.Portal>
  </Primitive.Root>;
}
