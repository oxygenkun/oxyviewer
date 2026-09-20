import type { SettingsDescriptor } from "@/types";

export function SettingsRenderer({ descriptor, values, disabled, onChange, valueLabel }: {
  descriptor: SettingsDescriptor;
  values: Record<string, string | number | boolean | undefined>;
  disabled?: boolean;
  valueLabel?: (key: string, value: string) => string;
  onChange: (key: string, value: string | number | boolean) => void;
}) {
  return <>{descriptor.fields.map((field) => <label className={field.type === "boolean" ? "people-panel__toggle" : "people-panel__field"} key={field.key}>
    <span>{field.label}</span>
    {field.type === "boolean" ? <input type="checkbox" disabled={disabled} checked={values[field.key] === true} onChange={(event) => onChange(field.key, event.target.checked)} />
      : field.type === "enum" ? <select disabled={disabled} value={String(values[field.key] ?? "")} onChange={(event) => onChange(field.key, event.target.value)}>{field.values.map((value) => <option key={value} value={value}>{valueLabel?.(field.key, value) ?? value}</option>)}</select>
      : <input type="number" disabled={disabled} min={field.min} max={field.max} step={field.step} value={Number(Number(values[field.key] ?? field.min).toFixed(3))} onChange={(event) => { if (event.target.value !== "" && event.target.validity.valid) onChange(field.key, event.target.valueAsNumber); }} />}
  </label>)}</>;
}
