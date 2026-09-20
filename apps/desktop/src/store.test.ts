// @vitest-environment jsdom
import { afterEach, expect, it } from "vitest";
import { useWorkspaceStore } from "./store";

afterEach(() => {
  localStorage.clear();
  useWorkspaceStore.setState({
    thumbnailOrientation: "landscape",
    thumbnailOrientations: {},
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
