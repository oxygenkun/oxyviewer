// @vitest-environment jsdom
import { afterEach, expect, it } from "vitest";
import { useWorkspaceStore } from "./store";

afterEach(() => {
  localStorage.clear();
  useWorkspaceStore.setState({
    thumbnailOrientation: "landscape",
    thumbnailOrientations: {},
    selectedIds: [],
    activeId: undefined,
  });
});

it("keeps burst grouping disabled by default", () => {
  expect(useWorkspaceStore.getInitialState().burstGroupingEnabled).toBe(false);
});

it("remembers and restores thumbnail orientation per folder", () => {
  const store = useWorkspaceStore.getState();
  store.setThumbnailOrientation("portrait", "/photos");
  store.setThumbnailOrientation("landscape", "/archive");

  store.restoreThumbnailOrientation("/photos");
  expect(useWorkspaceStore.getState().thumbnailOrientation).toBe("portrait");
  store.restoreThumbnailOrientation("/archive");
  expect(useWorkspaceStore.getState().thumbnailOrientation).toBe("landscape");

  store.forgetThumbnailOrientation("/photos");
  expect(useWorkspaceStore.getState().thumbnailOrientations).toEqual({ "/archive": "landscape" });
});

it("keeps the most recently selected photo focused during additive selection", () => {
  const store = useWorkspaceStore.getState();
  store.select("a");
  store.select("b", true);
  expect(useWorkspaceStore.getState()).toMatchObject({ selectedIds: ["a", "b"], activeId: "b" });

  useWorkspaceStore.getState().select("b", true);
  expect(useWorkspaceStore.getState()).toMatchObject({ selectedIds: ["a"], activeId: "a" });
});

it("selects a shift range and supports adding a range to the current selection", () => {
  const orderedIds = ["a", "b", "c", "d", "e"];
  const store = useWorkspaceStore.getState();
  store.select("b");
  store.selectRange(orderedIds, "d");
  expect(useWorkspaceStore.getState()).toMatchObject({ selectedIds: ["b", "c", "d"], activeId: "d" });

  useWorkspaceStore.getState().select("a", true);
  useWorkspaceStore.getState().selectRange(orderedIds, "c", true);
  expect(useWorkspaceStore.getState()).toMatchObject({ selectedIds: ["b", "c", "d", "a"], activeId: "c" });
});
