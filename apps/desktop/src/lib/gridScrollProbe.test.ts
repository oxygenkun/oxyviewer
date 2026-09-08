// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { runGridScrollProbe } from "./gridScrollProbe";
import { perfMark } from "./perfProbe";

vi.mock("./perfProbe", () => ({ perfMark: vi.fn() }));
vi.mock("../store", () => ({ useWorkspaceStore: { setState: vi.fn() } }));

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(perfMark).mockClear();
  document.body.innerHTML = `<div class="asset-scroll">
    <button class="asset-card"><span class="asset-card__name">a.hif</span>
      <div class="thumbnail"><img class="thumbnail__pending-image"></div>
    </button>
  </div>`;
  const scroller = document.querySelector<HTMLElement>(".asset-scroll")!;
  Object.defineProperties(scroller, {
    clientHeight: { value: 600 },
    scrollHeight: { value: 6000 },
  });
  scroller.getBoundingClientRect = () => ({ top: 0, bottom: 600 }) as DOMRect;
  const card = document.querySelector<HTMLElement>(".asset-card")!;
  card.getBoundingClientRect = () => ({ top: 0, bottom: 200 }) as DOMRect;
  const image = document.querySelector("img")!;
  Object.defineProperties(image, { complete: { value: true }, naturalWidth: { value: 160 } });
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

it("waits for the displayed image instead of counting a decoded but pending image", async () => {
  const controller = new AbortController();
  const run = runGridScrollProbe(controller.signal);
  await vi.advanceTimersByTimeAsync(1600);
  expect(vi.mocked(perfMark).mock.calls.some(([name]) => name === "grid-scroll:stopped")).toBe(true);
  expect(vi.mocked(perfMark).mock.calls.some(([name]) => name === "grid-scroll:ready")).toBe(false);
  document.querySelector("img")!.className = "";
  await vi.runAllTimersAsync();
  await run;
  const ready = vi.mocked(perfMark).mock.calls.filter(([name]) => name === "grid-scroll:ready");
  expect(ready).toHaveLength(3);
  expect(ready[0][1]).toMatchObject({ count: 1, assets: ["a.hif"] });
});

it("cancels a pending scroll without leaving timers or reporting completion", async () => {
  const controller = new AbortController();
  const run = runGridScrollProbe(controller.signal);
  const rejection = expect(run).rejects.toThrow();
  controller.abort();
  await rejection;
  expect(vi.getTimerCount()).toBe(0);
  expect(perfMark).not.toHaveBeenCalledWith("resource:stress-complete", expect.anything());
});
