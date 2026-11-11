import { shortcutFromEvent, type ShortcutInputEvent } from "./shortcuts";

interface Listener {
  handle: (event: ShortcutInputEvent) => void;
  capture: boolean;
}

const listeners = new Set<Listener>();
const consumedButtons = new Set<number>();

function dispatch(event: ShortcutInputEvent, capture: boolean) {
  for (const listener of listeners) {
    if (event.defaultPrevented) break;
    if (listener.capture === capture) listener.handle(event);
  }
}

function keyCapture(event: KeyboardEvent) { dispatch(event, true); }
function keyBubble(event: KeyboardEvent) { dispatch(event, false); }

function mouseInput(event: MouseEvent | WheelEvent) {
  if (event.type === "mousedown") consumedButtons.delete(event.button);
  if (event.defaultPrevented || !shortcutFromEvent(event)) return;
  dispatch(event, true);
  if (!event.defaultPrevented) dispatch(event, false);
  if (!event.defaultPrevented) return;
  // A bound wheel must run before React's loupe zoom/scroll handlers. Consumed
  // mouse releases must also suppress WebView history and middle-click actions.
  event.stopPropagation();
  if (event.type === "mousedown") consumedButtons.add(event.button);
}

function mouseRelease(event: MouseEvent) {
  if (!consumedButtons.has(event.button)) return;
  event.preventDefault();
  event.stopPropagation();
  if (event.type === "auxclick") {
    consumedButtons.delete(event.button);
    if (listeners.size === 0 && consumedButtons.size === 0) detachReleases();
  }
}

function resetButtons() {
  consumedButtons.clear();
  if (listeners.size === 0) detachReleases();
}

function detachReleases() {
  window.removeEventListener("mouseup", mouseRelease, true);
  window.removeEventListener("auxclick", mouseRelease, true);
  window.removeEventListener("blur", resetButtons);
}

function attach() {
  window.addEventListener("keydown", keyCapture, true);
  window.addEventListener("keydown", keyBubble);
  window.addEventListener("mousedown", mouseInput, true);
  window.addEventListener("wheel", mouseInput, { capture: true, passive: false });
  window.addEventListener("mouseup", mouseRelease, true);
  window.addEventListener("auxclick", mouseRelease, true);
  window.addEventListener("blur", resetButtons);
}

function detach() {
  window.removeEventListener("keydown", keyCapture, true);
  window.removeEventListener("keydown", keyBubble);
  window.removeEventListener("mousedown", mouseInput, true);
  window.removeEventListener("wheel", mouseInput, true);
  if (consumedButtons.size === 0) detachReleases();
}

// One shared dispatcher keeps consumed releases across view changes and effect
// re-subscriptions. Only matched inputs are canceled; unbound wheels still scroll.
export function listenShortcutInput(
  handle: Listener["handle"],
  { capture = false }: { capture?: boolean } = {},
): () => void {
  if (listeners.size === 0) attach();
  const listener = { handle, capture };
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) detach();
  };
}
