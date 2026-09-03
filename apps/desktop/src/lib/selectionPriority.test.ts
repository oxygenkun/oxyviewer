import { describe, expect, it } from "vitest";
import { orderBySelectionPriority } from "./selectionPriority";

describe("selection-centered priority order", () => {
  it("puts selection first, then nearest right, nearest left, and the remaining sides", () => {
    const items = ["l3", "l2", "l1", "selected", "r1", "r2", "r3"];

    expect(orderBySelectionPriority(items, "selected", (item) => item))
      .toEqual(["selected", "r1", "l1", "r2", "r3", "l2", "l3"]);
  });

  it("preserves input order when the selection is absent", () => {
    const items = ["one", "two", "three"];

    expect(orderBySelectionPriority(items, "missing", (item) => item)).toEqual(items);
  });
});
