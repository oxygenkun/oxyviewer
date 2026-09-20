// @vitest-environment jsdom
import { expect, it } from "vitest";
import { useWorkspaceStore } from "./store";

it("keeps burst grouping disabled by default", () => {
  expect(useWorkspaceStore.getInitialState().burstGroupingEnabled).toBe(false);
});
