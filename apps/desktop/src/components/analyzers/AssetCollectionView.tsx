import { useState, type ReactNode } from "react";
import type { CollectionViewDescriptor } from "@/types";

export function CollectionFields({ descriptor, values }: {
  descriptor: CollectionViewDescriptor;
  values: Record<string, string | number | undefined>;
}) {
  return <>{descriptor.fields.map((field) => {
    const value = values[field.key];
    const formatted = field.kind === "percent" && typeof value === "number"
      ? `${(value * 100).toFixed(0)}%`
      : value ?? "—";
    return <span key={field.key} className="people-panel__badge">{field.label}: {formatted}</span>;
  })}</>;
}

interface AssetCollectionViewProps<T> {
  descriptor: CollectionViewDescriptor;
  sourceId: string;
  items: T[];
  itemId: (item: T) => string;
  renderItem: (item: T) => ReactNode;
  actions: Record<string, { label: string; run: (ids: string[]) => Promise<unknown> }>;
  hasMore: boolean;
  loadMore: () => unknown;
  loading: boolean;
  labels: { more: string; select: string };
}

/** Only the supplied host registry can resolve actions; no arbitrary RPC names. */
export function AssetCollectionView<T>({
  descriptor, sourceId, items, itemId, renderItem, actions, hasMore, loadMore, loading, labels,
}: AssetCollectionViewProps<T>) {
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  if (descriptor.source !== sourceId) return <p role="alert">Unknown collection source</p>;

  const visible = new Set(items.map(itemId));
  const active = [...selected].filter((id) => visible.has(id));
  const perform = async (id: string) => {
    const action = Object.hasOwn(actions, id) ? actions[id] : undefined;
    if (!action || !descriptor.actions.includes(id) || busy || active.length === 0) return;
    setBusy(true);
    setError(undefined);
    try {
      await action.run(active);
      setSelected(new Set());
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  };

  return <>
    <div className="analyzer-collection__actions">
      {descriptor.actions.filter((id) => Object.hasOwn(actions, id)).map((id) => (
        <button key={id} disabled={busy || active.length === 0} type="button" onClick={() => void perform(id)}>
          {actions[id].label} ({active.length})
        </button>
      ))}
    </div>
    {error ? <p role="alert">{error}</p> : null}
    <ul className="people-panel__review">
      {items.map((item) => {
        const id = itemId(item);
        return <li key={id}>
          <label>
            <input
              type="checkbox"
              aria-label={`${labels.select} ${id}`}
              checked={selected.has(id)}
              disabled={busy}
              onChange={(event) => {
                const checked = event.target.checked;
                setSelected((before) => {
                  const next = descriptor.selection === "single" ? new Set<string>() : new Set(before);
                  if (checked) next.add(id);
                  else next.delete(id);
                  return next;
                });
              }}
            />
          </label>
          {renderItem(item)}
        </li>;
      })}
    </ul>
    {hasMore ? <button disabled={loading} type="button" onClick={loadMore}>{labels.more}</button> : null}
  </>;
}
