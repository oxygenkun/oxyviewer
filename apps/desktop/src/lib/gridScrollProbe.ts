import { perfMark } from "./perfProbe";
import { useWorkspaceStore } from "../store";

/** Explicit native performance scenario: continuous scrolling, then whole-viewport readiness. */
export async function runGridScrollProbe(signal: AbortSignal, horizontal = false): Promise<void> {
  const prefix = horizontal ? "filmstrip-scroll" : "grid-scroll";
  const wait = (ms: number) => new Promise<void>((resolve, reject) => {
    signal.throwIfAborted();
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = window.setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, ms);
    signal.addEventListener("abort", abort, { once: true });
  });
  useWorkspaceStore.setState({ view: horizontal ? "loupe" : "grid", thumbnailOrientation: "portrait", inspectorOpen: false });
  await wait(500);
  for (const [phase, fraction] of [0.45, 1, 0.2].entries()) {
    const scroller = document.querySelector<HTMLElement>(horizontal ? ".filmstrip" : ".asset-scroll");
    if (!scroller) throw new Error(`${prefix} surface missing`);
    const visibleCards = () => {
      const viewport = scroller.getBoundingClientRect();
      return Array.from(scroller.querySelectorAll<HTMLElement>(horizontal ? "[data-filmstrip-asset-id]" : ".asset-card"))
        .filter((card) => {
          const bounds = card.getBoundingClientRect();
          return horizontal ? bounds.right > viewport.left && bounds.left < viewport.right
            : bounds.bottom > viewport.top && bounds.top < viewport.bottom;
        });
    };
    const hasDisplayedImage = (card: HTMLElement) => {
      const image = card.querySelector<HTMLImageElement>(".thumbnail img:not(.thumbnail__pending-image)");
      return Boolean(image?.complete && image.naturalWidth > 0);
    };
    const start = horizontal ? scroller.scrollLeft : scroller.scrollTop;
    perfMark(`${prefix}:start`, { phase });
    for (let step = 1; step <= 32; step += 1) {
      const target = (horizontal ? scroller.scrollWidth - scroller.clientWidth
        : scroller.scrollHeight - scroller.clientHeight) * fraction;
      if (horizontal) scroller.scrollLeft = start + (target - start) * step / 32;
      else scroller.scrollTop = start + (target - start) * step / 32;
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
      await wait(16);
      if (step % 8 === 0) {
        const cards = visibleCards();
        perfMark(`${prefix}:moving`, { phase, step, count: cards.length,
          displayed: cards.filter(hasDisplayedImage).length });
      }
    }
    const stoppedAt = performance.now();
    perfMark(`${prefix}:stopped`, { phase, scrollOffset: horizontal ? scroller.scrollLeft : scroller.scrollTop });
    let ready = false;
    let firstReadyMs: number | undefined;
    while (performance.now() - stoppedAt < 15_000) {
      await wait(25);
      const cards = visibleCards();
      const painted = cards.filter(hasDisplayedImage);
      if (painted.length && firstReadyMs === undefined) firstReadyMs = performance.now() - stoppedAt;
      if (cards.length && painted.length === cards.length
        && !scroller.querySelector(horizontal ? ".filmstrip__placeholder" : ".asset-card-placeholder")) {
        perfMark(`${prefix}:ready`, {
          phase, count: cards.length, firstReadyMs, allReadyMs: performance.now() - stoppedAt,
          assets: cards.map((card) => card.querySelector(horizontal ? ".filmstrip__name" : ".asset-card__name")?.textContent),
        });
        ready = true;
        break;
      }
    }
    if (!ready) throw new Error(`${prefix} phase ${phase} did not finish painting within 15 seconds`);
  }
  perfMark("resource:stress-complete", { mode: prefix });
}
