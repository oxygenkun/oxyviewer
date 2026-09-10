import { describe, expect, it, vi } from "vitest";
import { orderedPriorityWeight, SerialTaskQueue, priorityWeight } from "./previewQueue";

describe("priorityWeight", () => {
  it("maps loupe above visible above nearby above filtered preload", () => {
    expect(priorityWeight("loupe")).toBeGreaterThan(priorityWeight("visible"));
    expect(priorityWeight("visible")).toBeGreaterThan(priorityWeight("nearby"));
    expect(priorityWeight("nearby")).toBeGreaterThan(priorityWeight("preload"));
  });

  it("keeps selection order inside a tier without crossing the next tier", () => {
    expect(orderedPriorityWeight("visible", 0))
      .toBeGreaterThan(orderedPriorityWeight("visible", 10));
    expect(orderedPriorityWeight("nearby", 0))
      .toBeGreaterThan(orderedPriorityWeight("preload", 0));
    expect(orderedPriorityWeight("visible", 999_999))
      .toBeGreaterThan(orderedPriorityWeight("nearby", 0));
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

  it("uses selection-centered order for work in the same tier", async () => {
    const queue = new SerialTaskQueue();
    let release!: () => void;
    const first = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    const order: string[] = [];
    const left = queue.enqueue(orderedPriorityWeight("visible", 2), undefined, async () => {
      order.push("left");
    });
    const remainingRight = queue.enqueue(
      orderedPriorityWeight("visible", 3),
      undefined,
      async () => {
        order.push("remaining-right");
      },
    );
    const selected = queue.enqueue(orderedPriorityWeight("visible", 0), undefined, async () => {
      order.push("selected");
    });
    const right = queue.enqueue(orderedPriorityWeight("visible", 1), undefined, async () => {
      order.push("right");
    });

    release();
    await Promise.all([first, left, remainingRight, selected, right]);
    expect(order).toEqual(["selected", "right", "left", "remaining-right"]);
  });

  it("promotes the same pending artifact without creating a second task", async () => {
    const queue = new SerialTaskQueue();
    let release!: () => void;
    const first = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    const order: string[] = [];
    let effectivePriority = -1;
    const shared = queue.enqueue(0, undefined, async (priority) => {
      effectivePriority = priority;
      order.push("shared");
    }, "asset:thumbnail");
    const visible = queue.enqueue(1, undefined, async () => {
      order.push("visible");
    });

    expect(queue.raisePriority("asset:thumbnail", 2)).toBe(true);
    release();
    await Promise.all([first, shared, visible]);

    expect(order).toEqual(["shared", "visible"]);
    expect(effectivePriority).toBe(2);
  });
});

it("allows another transfer to finish while an older transfer is stalled", async () => {
  const queue = new SerialTaskQueue(2);
  let release!: () => void;
  const blocked = queue.enqueue(0, undefined, () => new Promise<void>((resolve) => { release = resolve; }));
  const selected = vi.fn(async () => undefined);
  await queue.enqueue(priorityWeight("loupe"), undefined, selected);
  expect(selected).toHaveBeenCalledOnce();
  release();
  await blocked;
});
