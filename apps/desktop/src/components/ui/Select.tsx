import { Check, ChevronDown } from "lucide-react";
import type { ReactNode } from "react";
import { Button, IconButton } from "./Button";
import * as Menu from "./DropdownMenu";
import styles from "./Select.module.css";

interface SelectProps<T extends string> {
  label: string;
  value: T;
  options: ReadonlyArray<{ value: T; label: string; disabled?: boolean }>;
  onValueChange: (value: T) => void;
  disabled?: boolean;
  icon?: ReactNode;
  size?: "small" | "regular";
  className?: string;
  container?: HTMLElement | null;
}

/** A single-choice settings menu, sharing keyboard/focus behavior and menu styles. */
export function Select<T extends string>({ label, value, options, onValueChange, disabled,
  icon, size = "regular", className, container }: SelectProps<T>) {
  const selected = options.find(option => option.value === value);
  return <Menu.Root modal={false}>
    <Menu.Trigger asChild>
      {icon ? <IconButton label={label} size={size} disabled={disabled} className={className}
        title={`${label}: ${selected?.label ?? ""}`}>{icon}</IconButton>
        : <Button size={size} disabled={disabled} aria-label={label} title={selected?.label}
          className={[styles.trigger, className].filter(Boolean).join(" ")}>
          <span>{selected?.label}</span><ChevronDown size={13} />
        </Button>}
    </Menu.Trigger>
    <Menu.Content container={container} aria-label={label} align="end" className={styles.content}>
      <Menu.RadioGroup value={value} onValueChange={next => {
        const option = options.find(option => option.value === next);
        if (option && !option.disabled) onValueChange(option.value);
      }}>
        {options.map(option => <Menu.RadioItem key={option.value} value={option.value} disabled={option.disabled}>
          <span className={styles.indicator}><Menu.ItemIndicator><Check size={13} /></Menu.ItemIndicator></span>
          <span className={styles.label}>{option.label}</span>
        </Menu.RadioItem>)}
      </Menu.RadioGroup>
    </Menu.Content>
  </Menu.Root>;
}
