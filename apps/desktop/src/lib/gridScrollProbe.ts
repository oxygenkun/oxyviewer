import { perfMark } from "./perfProbe";
import { useWorkspaceStore } from "../store";

/** Explicit native performance scenario: continuous scrolling, then whole-viewport readiness. */
export async function runGridScrollProbe(signal: AbortSignal): Promise<void> {
  const wait = (ms: number) => new Promise<void>((resolve, reject) => {
    signal.throwIfAborted();
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = window.setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, ms);
    signal.addEventListener("abort", abort, { once: true });
  });
  useWorkspaceStore.setState({ view: "grid", thumbnailOrientation: "portrait", inspectorOpen: false });
  await wait(500);
  for (const [phase, fraction] of [0.45, 1, 0.2].entries()) {
    const scroller = document.querySelector<HTMLElement>(".asset-scroll");
    if (!scroller) throw new Error("Grid scroll surface missing");
    const start = scroller.scrollTop;
    perfMark("grid-scroll:start", { phase });
    for (let step = 1; step <= 32; step += 1) {
      const target = (scroller.scrollHeight - scroller.clientHeight) * fraction;
      scroller.scrollTop = start + (target - start) * step / 32;
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
      await wait(16);
    }
    const stoppedAt = performance.now();
    perfMark("grid-scroll:stopped", { phase, scrollTop: scroller.scrollTop });
    let ready = false;
    let firstReadyMs: number | undefined;
    while (performance.now() - stoppedAt < 15_000) {
      await wait(25);
      const viewport = scroller.getBoundingClientRect();
      const cards = Array.from(scroller.querySelectorAll<HTMLElement>(".asset-card"))
        .filter((card) => {
          const bounds = card.getBoundingClientRect();
          return bounds.bottom > viewport.top && bounds.top < viewport.bottom;
        });
      const painted = cards.filter((card) => {
        const image = card.querySelector<HTMLImageElement>(".thumbnail > img:not(.thumbnail__pending-image)");
        return image?.complete && image.naturalWidth > 0;
      });
      if (painted.length && firstReadyMs === undefined) firstReadyMs = performance.now() - stoppedAt;
      if (cards.length && painted.length === cards.length
        && !scroller.querySelector(".asset-card-placeholder")) {
        perfMark("grid-scroll:ready", {
          phase, count: cards.length, firstReadyMs, allReadyMs: performance.now() - stoppedAt,
          assets: cards.map((card) => card.querySelector(".asset-card__name")?.textContent),
        });
        ready = true;
        break;
      }
    }
    if (!ready) throw new Error(`Grid phase ${phase} did not finish painting within 15 seconds`);
  }
  perfMark("resource:stress-complete", { mode: "grid-scroll" });
}
