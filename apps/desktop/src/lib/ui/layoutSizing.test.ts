import { describe, expect, it } from "vitest";
import { LAYOUT_SIZE_LIMITS, maxInspectorWidth, MIN_WORKSPACE_WIDTH } from "./layoutSizing";

describe("maxInspectorWidth", () => {
  it("preserves the configured maximum when the workspace has room", () => {
    expect(maxInspectorWidth(1_400, 224)).toBe(LAYOUT_SIZE_LIMITS.inspector.max);
  });

  it("leaves the workspace minimum width available as the shell narrows", () => {
    expect(maxInspectorWidth(900, 224)).toBe(900 - 224 - MIN_WORKSPACE_WIDTH);
  });

  it("never lowers the inspector below its minimum width", () => {
    expect(maxInspectorWidth(600, 224)).toBe(LAYOUT_SIZE_LIMITS.inspector.min);
  });
});
