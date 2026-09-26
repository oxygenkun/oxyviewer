import { LoaderCircle } from "lucide-react";
import type { ComponentProps } from "react";
import styles from "./Button.module.css";

export interface ButtonProps extends ComponentProps<"button"> {
  variant?: "secondary" | "primary" | "danger" | "ghost";
  size?: "small" | "regular";
  loading?: boolean;
}

export function Button({ variant = "secondary", size = "regular", loading = false,
  disabled, className, children, type = "button", ...props }: ButtonProps) {
  return <button {...props} type={type} disabled={disabled || loading}
    aria-busy={loading || undefined}
    className={[styles.button, styles[variant], styles[size], className].filter(Boolean).join(" ")}>
    {loading ? <LoaderCircle size={14} className={styles.spinner} aria-hidden="true" /> : null}
    {children}
  </button>;
}

export function IconButton({ label, className, variant = "ghost", ...props }:
  Omit<ButtonProps, "aria-label"> & { label: string }) {
  return <Button {...props} variant={variant} aria-label={label}
    className={[styles.icon, className].filter(Boolean).join(" ")} />;
}
