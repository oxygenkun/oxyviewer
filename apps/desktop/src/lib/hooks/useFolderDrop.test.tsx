// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { WindowDragEvent } from "@/types";
import { useFolderDrop } from "./useFolderDrop";

// Exercise the native branch: jsdom has no Tauri, so the module is stubbed the
// way the webview API behaves on the desktop.
const native = vi.hoisted(() => ({
  handlers: [] as Array<(event: WindowDragEvent) => void>,
  disposed: 0,
}));

vi.mock("@/lib/api", () => ({
  isTauri: () => true,
  onWindowDragDrop: async (callback: (event: WindowDragEvent) => void) => {
    native.handlers.push(callback);
    return () => {
      native.disposed += 1;
    };
  },
}));

let root: Root;
let container: HTMLDivElement;
let renders = 0;
const dropped: string[][] = [];

function Probe() {
  const state = useFolderDrop((paths) => dropped.push(paths));
  renders += 1;
  return (
    <div
      data-testid="overlay"
      data-visible={state.visible}
      data-names={state.folderNames.join(",")}
      data-count={state.itemCount}
    />
  );
}

const overlay = () => container.querySelector<HTMLDivElement>('[data-testid="overlay"]')!;
const emit = async (event: WindowDragEvent) => {
  await act(async () => {
    for (const handler of native.handlers) handler(event);
  });
};

beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  native.handlers.length = 0;
  native.disposed = 0;
  renders = 0;
  dropped.length = 0;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => root.render(<Probe />));
  // The listener subscription resolves on a microtask.
  await act(async () => {});
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

it("subscribes to the native drag channel and unsubscribes on unmount", async () => {
  expect(native.handlers).toHaveLength(1);
  await act(async () => root.unmount());
  root = createRoot(container);
  expect(native.disposed).toBe(1);
});

it("shows the preview on enter, ignores pointer-frequency over events, and drops once", async () => {
  expect(overlay().dataset.visible).toBe("false");

  await emit({
    type: "enter",
    paths: ["/photos/2024", "/photos/trips", "/photos/a", "/photos/b", "/photos/c"],
    position: { x: 20, y: 40 },
  });
  expect(overlay().dataset.visible).toBe("true");
  expect(overlay().dataset.names).toBe("2024,trips,a,b");
  expect(overlay().dataset.count).toBe("5");

  // `over` repeats at pointer frequency and has nothing to update: the card
  // covers the window, so it must neither re-render nor change the preview.
  const rendersAfterEnter = renders;
  for (const position of [{ x: 120, y: 90 }, { x: 240, y: 300 }, { x: 10, y: 10 }]) {
    await emit({ type: "over", position });
  }
  expect(renders).toBe(rendersAfterEnter);
  expect(overlay().dataset.visible).toBe("true");
  expect(overlay().dataset.names).toBe("2024,trips,a,b");

  await emit({ type: "drop", paths: ["/photos/2024"], position: { x: 120, y: 90 } });
  expect(overlay().dataset.visible).toBe("false");
  expect(dropped).toEqual([["/photos/2024"]]);
});

it("hides the preview when the drag leaves the window", async () => {
  await emit({ type: "enter", paths: ["/photos/2024"], position: { x: 5, y: 5 } });
  expect(overlay().dataset.visible).toBe("true");

  await emit({ type: "leave" });
  expect(overlay().dataset.visible).toBe("false");
  expect(dropped).toEqual([]);
});
