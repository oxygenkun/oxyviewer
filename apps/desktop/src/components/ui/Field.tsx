import { useId, type ComponentProps, type ReactNode } from "react";
import styles from "./Field.module.css";

interface FieldProps extends Omit<ComponentProps<"input">, "size"> {
  label: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  suffix?: ReactNode;
}

/** A labelled input with linked help/error text. Groups of controls use fieldset. */
export function Field({ label, hint, error, suffix, id: suppliedId, className,
  "aria-describedby": describedBy, "aria-invalid": invalid, ...props }: FieldProps) {
  const generatedId = useId();
  const id = suppliedId ?? generatedId;
  const description = [describedBy, hint ? `${id}-hint` : undefined,
    error ? `${id}-error` : undefined].filter(Boolean).join(" ") || undefined;
  return <div className={[styles.field, className].filter(Boolean).join(" ")}>
    <label htmlFor={id}>{label}</label>
    <div className={styles.control}>
      <input {...props} id={id} aria-describedby={description} aria-invalid={error ? true : invalid} />
      {suffix ? <span>{suffix}</span> : null}
    </div>
    {hint ? <small id={`${id}-hint`}>{hint}</small> : null}
    {error ? <small id={`${id}-error`} className={styles.error} role="alert">{error}</small> : null}
  </div>;
}
