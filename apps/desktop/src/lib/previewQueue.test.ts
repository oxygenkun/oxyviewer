import { describe, expect, it, vi } from "vitest";
import { SerialTaskQueue, priorityWeight } from "./previewQueue";

describe("priorityWeight", () => {
  it("maps loupe above visible above nearby above filtered preload", () => {
    expect(priorityWeight("loupe")).toBeGreaterThan(priorityWeight("visible"));
    expect(priorityWeight("visible")).toBeGreaterThan(priorityWeight("nearby"));
    expect(priorityWeight("nearby")).toBeGreaterThan(priorityWeight("preload"));
  });
});

describe("serial preview task queue", () => {
  it("does not start a second task while the first is active", async () => {
    const queue = new SerialTaskQueue();
    let release!: () => void;
    const first = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    let secondStarted = false;
    const second = queue.enqueue(0, undefined, async () => {
      secondStarted = true;
    });

    await Promise.resolve();
    expect(secondStarted).toBe(false);
    release();
    await Promise.all([first, second]);
    expect(secondStarted).toBe(true);
  });

  it("drops an aborted task before it starts", async () => {
    const queue = new SerialTaskQueue();
    let release!: () => void;
    const first = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    const controller = new AbortController();
    const run = vi.fn(async () => undefined);
    const second = queue.enqueue(0, controller.signal, run);
    controller.abort();
    release();

    await first;
    await expect(second).rejects.toMatchObject({ name: "AbortError" });
    expect(run).not.toHaveBeenCalled();
  });

  it("runs higher priority queued work first across four tiers", async () => {
    // Unified queue: a loupe request arriving last still wins, then visible,
    // then nearby — regardless of which format/stage produced the request.
    const queue = new SerialTaskQueue();
    let release!: () => void;
    const first = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    const order: string[] = [];
    const preload = queue.enqueue(priorityWeight("preload"), undefined, async () => {
      order.push("preload");
    });
    const nearby = queue.enqueue(priorityWeight("nearby"), undefined, async () => {
      order.push("nearby");
    });
    const visible = queue.enqueue(priorityWeight("visible"), undefined, async () => {
      order.push("visible");
    });
    const loupe = queue.enqueue(priorityWeight("loupe"), undefined, async () => {
      order.push("loupe");
    });

    release();
    await Promise.all([first, preload, nearby, visible, loupe]);
    expect(order).toEqual(["loupe", "visible", "nearby", "preload"]);
  });
});
