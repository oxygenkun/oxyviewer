// Native file drags outlive the command's main-thread dispatch on macOS/GTK.
// The API holds this session until the OS reports a drop or cancellation.
let active = false;
const listeners = new Set<() => void>();

export const isAssetDragActive = () => active;
export function subscribeAssetDragSession(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function beginAssetDragSession(): () => void {
  if (active) throw new Error("A file drag is already active");
  active = true;
  listeners.forEach((listener) => listener());
  let finished = false;
  return () => {
    if (finished) return;
    finished = true;
    active = false;
    listeners.forEach((listener) => listener());
  };
}
