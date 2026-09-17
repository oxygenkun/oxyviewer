import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AssetSummary } from "@/types";
import {
  reconcilePreviewSchedule,
  releasePreviewSchedule,
  upsertPreviewSchedule,
} from "@/lib/api";
import {
  backgroundPreviewIntents,
  filmstripPreviewIntents,
  PreviewScheduleScope,
  viewportPreviewIntents,
} from "./previewScheduling";

vi.mock("@/lib/api", () => ({
  reconcilePreviewSchedule: vi.fn(() => Promise.resolve(true)),
  releasePreviewSchedule: vi.fn(() => Promise.resolve(true)),
  upsertPreviewSchedule: vi.fn(() => Promise.resolve(true)),
}));

function asset(id: string): AssetSummary {
  return {
    id,
    path: `/photos/${id}.arw`,
    name: `${id}.arw`,
    extension: "arw",
    kind: "raw",
    sizeBytes: 1,
    modifiedAtMs: 1,
    hasSidecar: false,
  };
}

describe("preview scheduling policy", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => (
      setTimeout(() => callback(performance.now()), 0) as unknown as number
    ));
    vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
    vi.mocked(reconcilePreviewSchedule).mockClear();
    vi.mocked(releasePreviewSchedule).mockClear();
    vi.mocked(upsertPreviewSchedule).mockClear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("places selection first, then visible and nearby viewport work", () => {
    const selected = asset("selected");
    const intents = viewportPreviewIntents([
      { asset: asset("left"), visible: true, distance: 30 },
      { asset: selected, visible: true, distance: 0 },
      { asset: asset("right"), visible: true, distance: 20 },
      { asset: asset("nearby"), visible: false, distance: 10 },
    ], selected);

    expect(intents.map((intent) => [intent.path, intent.priority, intent.rank])).toEqual([
      ["/photos/selected.arw", "loupe", 0],
      ["/photos/right.arw", "visible", 0],
      ["/photos/left.arw", "visible", 1],
      ["/photos/nearby.arw", "nearby", 0],
    ]);
  });

  it("does not keep an off-screen selection ahead of the new viewport", () => {
    const selected = asset("old-selection");
    const intents = viewportPreviewIntents([
      { asset: asset("new-visible"), visible: true, distance: 0 },
      { asset: selected, visible: false, distance: 100 },
    ], selected);

    expect(intents.map((intent) => [intent.path, intent.priority, intent.rank])).toEqual([
      ["/photos/new-visible.arw", "visible", 0],
      ["/photos/old-selection.arw", "nearby", 0],
    ]);
  });

  it("uses balanced selection-relative ordering for background work", () => {
    const intents = backgroundPreviewIntents(
      [asset("l2"), asset("l1"), asset("selected"), asset("r1")],
      "selected",
    );
    expect(intents.map((intent) => intent.path)).toEqual([
      "/photos/r1.arw",
      "/photos/l1.arw",
      "/photos/l2.arw",
    ]);
  });

  it("demotes filmstrip overscan while retaining the loupe selection", () => {
    const selected = asset("selected");
    const oldVisible = asset("old-visible");
    const newVisible = asset("new-visible");
    const intents = filmstripPreviewIntents([
      { asset: oldVisible, visible: false, distance: 200 },
      { asset: newVisible, visible: true, distance: 0 },
    ], selected);
    expect(intents.map(({ path, priority }) => [path, priority])).toEqual([
      [selected.path, "loupe"], [newVisible.path, "visible"], [oldVisible.path, "nearby"],
    ]);
    expect(filmstripPreviewIntents([
      { asset: selected, visible: true, distance: 0 },
    ], selected)).toHaveLength(1);
  });

  it("coalesces same-frame snapshots and skips identical content", async () => {
    const scope = new PreviewScheduleScope("test", { minDispatchIntervalMs: 50 });
    const first = viewportPreviewIntents([
      { asset: asset("a"), visible: true, distance: 0 },
    ], undefined);
    const latest = viewportPreviewIntents([
      { asset: asset("b"), visible: true, distance: 0 },
    ], undefined);

    scope.reconcile(first);
    scope.reconcile(latest);
    scope.reconcile(latest);
    await vi.advanceTimersByTimeAsync(0);

    expect(reconcilePreviewSchedule).toHaveBeenCalledTimes(1);
    expect(reconcilePreviewSchedule).toHaveBeenCalledWith(
      scope.id,
      1,
      latest,
      { action: "release" },
    );
  });

  it("rate-limits continuous viewport updates and keeps the newest snapshot", async () => {
    const scope = new PreviewScheduleScope("test", { minDispatchIntervalMs: 50 });
    const snapshot = (id: string) => viewportPreviewIntents([
      { asset: asset(id), visible: true, distance: 0 },
    ], undefined);

    scope.reconcile(snapshot("a"));
    await vi.advanceTimersByTimeAsync(0);
    scope.reconcile(snapshot("b"));
    scope.reconcile(snapshot("c"));

    await vi.advanceTimersByTimeAsync(49);
    expect(reconcilePreviewSchedule).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(reconcilePreviewSchedule).toHaveBeenCalledTimes(2);
    expect(reconcilePreviewSchedule).toHaveBeenLastCalledWith(
      scope.id,
      2,
      snapshot("c"),
      { action: "release" },
    );
  });

  it("applies bridge backpressure while retaining the latest pending snapshot", async () => {
    let resolveFirst: ((value: boolean) => void) | undefined;
    vi.mocked(reconcilePreviewSchedule).mockImplementationOnce(() => (
      new Promise((resolve) => { resolveFirst = resolve; })
    ));
    const scope = new PreviewScheduleScope("test", { minDispatchIntervalMs: 50 });
    const snapshot = (id: string) => viewportPreviewIntents([
      { asset: asset(id), visible: true, distance: 0 },
    ], undefined);

    scope.reconcile(snapshot("a"));
    await vi.advanceTimersByTimeAsync(0);
    scope.reconcile(snapshot("b"));
    scope.reconcile(snapshot("c"));
    await vi.advanceTimersByTimeAsync(500);
    expect(reconcilePreviewSchedule).toHaveBeenCalledTimes(1);

    resolveFirst?.(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(reconcilePreviewSchedule).toHaveBeenCalledTimes(2);
    expect(reconcilePreviewSchedule).toHaveBeenLastCalledWith(
      scope.id,
      2,
      snapshot("c"),
      { action: "release" },
    );
  });

  it("drops queued viewport updates when the scope is released", async () => {
    const scope = new PreviewScheduleScope("test", { minDispatchIntervalMs: 50 });
    scope.reconcile(viewportPreviewIntents([
      { asset: asset("a"), visible: true, distance: 0 },
    ], undefined));
    scope.release();
    await vi.advanceTimersByTimeAsync(0);

    expect(reconcilePreviewSchedule).toHaveBeenCalledTimes(1);
    expect(reconcilePreviewSchedule).toHaveBeenCalledWith(
      scope.id,
      1,
      [],
      { action: "release" },
    );
  });
});
