import { useWorkspaceStore } from "../store";
import type { AssetSummary } from "../types";
import { perfMark } from "./perfProbe";

/** Real-WebView geometry regression, enabled only by the isolated perf harness. */
export async function runLoupeZoomProbe(assets: readonly AssetSummary[], signal: AbortSignal): Promise<void> {
  if (assets[0]?.kind !== "raw") throw new Error("Zoom probe requires a RAW fixture");
  useWorkspaceStore.getState().select(assets[0].id);
  useWorkspaceStore.getState().setView("loupe");
  const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  const deadline = performance.now() + 30_000;
  let image: HTMLImageElement | null = null;
  while (performance.now() < deadline) {
    signal.throwIfAborted();
    image = document.querySelector<HTMLImageElement>(".loupe__render img:not(.thumbnail__pending-image)");
    if (image?.complete && image.naturalWidth > 1024
      && document.querySelector(".loupe__raw-status--fullReady")) break;
    await frame();
  }
  if (!image?.complete || image.naturalWidth <= 1024
    || !document.querySelector(".loupe__raw-status--fullReady")) {
    throw new Error("Full RAW image did not load");
  }
  const stage = document.querySelector<HTMLElement>(".loupe__stage")!;
  const render = document.querySelector<HTMLElement>(".loupe__render")!;
  for (const percent of [50, 73, 91, 100, 150, 200, 400]) {
    signal.throwIfAborted();
    const before = render.getBoundingClientRect();
    const targetWidth = image.naturalWidth * percent / 100;
    const bounds = stage.getBoundingClientRect();
    stage.dispatchEvent(new WheelEvent("wheel", {
      bubbles: true, cancelable: true,
      clientX: bounds.x + bounds.width / 2, clientY: bounds.y + bounds.height / 2,
      deltaY: -Math.log(targetWidth / before.width) / 0.002,
    }));
    await frame(); await frame();
    const box = image.getBoundingClientRect();
    const outer = render.getBoundingClientRect();
    const detail = { percent, width: box.width, height: box.height,
      renderWidth: outer.width, renderHeight: outer.height,
      naturalWidth: image.naturalWidth, naturalHeight: image.naturalHeight,
      objectFit: getComputedStyle(image).objectFit,
      styleWidth: render.style.width, styleHeight: render.style.height };
    perfMark("loupe:zoom-sample", detail);
    if (Math.abs(box.width - targetWidth) > 1
      || Math.abs(box.height - image.naturalHeight * percent / 100) > 1) {
      throw new Error(`Zoom geometry diverged: ${JSON.stringify(detail)}`);
    }
  }
  perfMark("resource:stress-complete", {
    mode: "loupe-zoom", assetName: document.querySelector(".loupe__caption > span")?.textContent,
  });
}
