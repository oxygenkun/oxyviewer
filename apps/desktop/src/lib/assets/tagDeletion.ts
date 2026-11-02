import { create } from "zustand";
import { UNDO_WINDOW_MS } from "@/lib/ui/undoWindow";

export interface PendingTagDeletion {
  id: number;
  /** `tag` removes the vocabulary entry with its children; `assignment` clears one photo. */
  kind: "tag" | "assignment";
  tagPath: string;
  tagIds: Set<number>;
  path?: string;
  status: "pending" | "committing" | "failed";
  error?: string;
}

type DeletionInput = Omit<PendingTagDeletion, "id" | "status" | "error">;
const timers = new Map<number, ReturnType<typeof setTimeout>>();
let nextId = 0;
export const useTagDeletions = create<{ requests: PendingTagDeletion[] }>(() => ({ requests: [] }));
const remove = (id: number) => useTagDeletions.setState(state => ({ requests: state.requests.filter(item => item.id !== id) }));

/** Delay destructive writes so undo preserves original IDs, sources and revisions. */
export function enqueueTagDeletion(input: DeletionInput, commit: () => Promise<void>) {
  const overlaps = useTagDeletions.getState().requests.some(item => item.status !== "failed" &&
    (item.kind === "tag" || input.kind === "tag" || item.path === input.path) &&
    [...input.tagIds].some(id => item.tagIds.has(id)));
  if (overlaps) return;
  const id = nextId++;
  useTagDeletions.setState(state => ({ requests: [...state.requests, { ...input, id, status: "pending" }] }));
  timers.set(id, setTimeout(() => {
    timers.delete(id);
    useTagDeletions.setState(state => ({ requests: state.requests.map(item => item.id === id ? { ...item, status: "committing" } : item) }));
    void Promise.resolve().then(commit).then(() => remove(id), error => {
      useTagDeletions.setState(state => ({ requests: state.requests.map(item => item.id === id
        ? { ...item, status: "failed", error: String(error) } : item) }));
    });
  }, UNDO_WINDOW_MS));
}

export function undoTagDeletion(id: number) {
  const request = useTagDeletions.getState().requests.find(item => item.id === id);
  if (!request || request.status === "committing") return;
  clearTimeout(timers.get(id));
  timers.delete(id);
  remove(id);
}

/** App shutdown cancels writes whose undo window has not expired. */
export function cancelPendingTagDeletions() {
  for (const id of timers.keys()) undoTagDeletion(id);
}

/** Tags a pending vocabulary deletion will remove from every photo, including children. */
export function pendingDeletedTagIds(requests: PendingTagDeletion[]): Set<number> {
  const ids = new Set<number>();
  for (const request of requests) {
    if (request.kind !== "tag" || request.status === "failed") continue;
    for (const id of request.tagIds) ids.add(id);
  }
  return ids;
}

/** True while a queued removal still has to delete this tag from the photo. */
export function hasPendingRemoval(requests: PendingTagDeletion[], path: string, tagId: number): boolean {
  return requests.some(request => request.kind === "assignment" && request.status !== "failed" &&
    request.path === path && request.tagIds.has(tagId));
}
