import { describe, expect, it, vi } from "vitest";
import { SerialTaskQueue } from "./previewQueue";

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

  it("runs higher priority queued work first", async () => {
    const queue = new SerialTaskQueue();
    let release!: () => void;
    const first = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    const order: string[] = [];
    const nearby = queue.enqueue(0, undefined, async () => {
      order.push("nearby");
    });
    const visible = queue.enqueue(1, undefined, async () => {
      order.push("visible");
    });

    release();
    await Promise.all([first, nearby, visible]);
    expect(order).toEqual(["visible", "nearby"]);
  });
});
