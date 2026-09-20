import { describe, expect, it } from "vitest";
import { burstAdjustedTotal, collapsedBurstPaths } from "./burstGroups";
import type { BurstGroup } from "@/types";

const group = (representative: string, members: string[]): BurstGroup => ({
  representative,
  members,
});

describe("collapsedBurstPaths", () => {
  it("hides every member but the representative", () => {
    const hidden = collapsedBurstPaths(
      [group("/shoot/A.ARW", ["/shoot/A.ARW", "/shoot/B.ARW", "/shoot/C.ARW"])],
      new Set(),
    );
    expect([...hidden]).toEqual(["/shoot/B.ARW", "/shoot/C.ARW"]);
  });

  it("keeps the members of an expanded group visible", () => {
    const groups = [
      group("/shoot/A.ARW", ["/shoot/A.ARW", "/shoot/B.ARW"]),
      group("/shoot/D.ARW", ["/shoot/D.ARW", "/shoot/E.ARW"]),
    ];
    const hidden = collapsedBurstPaths(groups, new Set(["/shoot/A.ARW"]));
    expect([...hidden]).toEqual(["/shoot/E.ARW"]);
  });

  it("hides nothing without groups", () => {
    expect(collapsedBurstPaths([], new Set()).size).toBe(0);
  });

  it("hides nothing when burst grouping is disabled", () => {
    const groups = [group("/shoot/A.ARW", ["/shoot/A.ARW", "/shoot/B.ARW"])];
    expect(collapsedBurstPaths(groups, new Set(), false).size).toBe(0);
  });

  it("leaves a one-frame run alone", () => {
    const hidden = collapsedBurstPaths([group("/shoot/A.ARW", ["/shoot/A.ARW"])], new Set());
    expect(hidden.size).toBe(0);
  });
});

describe("burstAdjustedTotal", () => {
  it("subtracts collapsed members discovered in loaded pages", () => {
    expect(burstAdjustedTotal(1_000, 250, 230)).toBe(980);
  });

  it("uses the exact visible count when every page is loaded", () => {
    expect(burstAdjustedTotal(24, 24, 15)).toBe(15);
  });

  it("never reports fewer items than are currently visible", () => {
    expect(burstAdjustedTotal(10, 12, 12)).toBe(12);
  });
});
