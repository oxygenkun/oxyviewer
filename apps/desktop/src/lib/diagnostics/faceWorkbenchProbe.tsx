import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { FacePhotoReview } from "@/components/people/FacePhotoReview";
import { addLibraryRoot, getFaceCapability, getFaceClusters, getFaceReviewPage, getMediaResourceStats, listPersons, onLibraryIndexUpdated, startFaceAnalysis } from "@/lib/api";
import { translate } from "@/lib/i18n";
import type { AssetSummary } from "@/types";
import { perfMark } from "./perfProbe";

/** Only invoked by an explicit isolated Release E2E scenario. */
export async function runFaceWorkbenchProbe(assets: AssetSummary[], signal: AbortSignal): Promise<void> {
  const until = async (ready: () => boolean | Promise<boolean>, timeout = 30000) => {
    const deadline = performance.now() + timeout;
    while (!await ready()) {
      signal.throwIfAborted();
      if (performance.now() > deadline) throw new Error("Face workbench probe timed out");
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
  };
  const folder = assets[0].path.replace(/[\\/][^\\/]+$/, "");
  let indexed = false;
  const stop = await onLibraryIndexUpdated((event) => { if (event.rootPath === folder && event.assetCount >= assets.length) indexed = true; });
  try { await addLibraryRoot(folder); await until(() => indexed); } finally { stop(); }
  await startFaceAnalysis({ paths: assets.map((asset) => asset.path), force: true });
  await until(async () => !(await getFaceCapability()).running, 120000);
  const page = await getFaceReviewPage(0, 200);
  if (page.items.length < 2) throw new Error("Fixture did not produce multiple faces");
  const clusters = await getFaceClusters();
  const persons = await listPersons();
  const host = document.createElement("div");
  host.className = "face-workbench";
  Object.assign(host.style, { position: "fixed", inset: "0", zIndex: "99999", overflow: "auto", padding: "20px", display: "block" });
  document.body.append(host);
  const root = createRoot(host);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  try {
    const started = performance.now();
    root.render(<QueryClientProvider client={client}><FacePhotoReview persons={persons} clusters={clusters} t={(key) => translate("en", key)} onReveal={() => {}} invalidate={() => {}} /></QueryClientProvider>);
    await until(() => [...host.querySelectorAll<HTMLImageElement>(".face-photo__canvas img")].some((image) => image.complete && image.naturalWidth > 0)
      && host.querySelectorAll(".face-photo__region").length >= 2);
    const elapsedMs = performance.now() - started;
    const image = host.querySelector<HTMLImageElement>(".face-photo__canvas img")!;
    if (image.naturalWidth > 2048 || image.naturalHeight > 2048) throw new Error("Workbench requested an unbounded image");
    if (!image.src.includes("oxy-media")) throw new Error("Preview did not use registered media protocol");
    perfMark("faces:workbench-painted", { elapsedMs, width: image.naturalWidth, height: image.naturalHeight, boxes: host.querySelectorAll(".face-photo__region").length });
    if (elapsedMs > 800) throw new Error(`Workbench exceeded preview guardrail: ${elapsedMs} ms`);
    const click = (label: string) => {
      const button = [...host.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === label);
      if (!button) throw new Error(`Missing workbench button: ${label}`);
      button.click();
    };
    click("Select loaded results");
    await until(() => host.querySelectorAll('.face-photo__face.is-selected').length === page.items.length);
    click("Automatic clusters");
    await until(() => host.querySelectorAll('.face-photo__face.is-selected').length === 0 && Boolean(host.querySelector(".is-grouped")));
    click("By review status");
    await until(() => Boolean(host.querySelector(".face-photos__group > header")));
    click("Select loaded results");
    await until(() => host.querySelectorAll('.face-photo__face.is-selected').length === page.items.length);
    const input = host.querySelector<HTMLInputElement>('.face-photo__actions input')!;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Workbench fixture");
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await until(() => ![...host.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === translate("en", "peopleCreateAndAssignIdentity"))?.disabled);
    // Use the rendered translation, so changes to copy do not change the action.
    click(translate("en", "peopleCreateAndAssignIdentity"));
    await until(() => host.querySelector('.face-photos__group > header')?.textContent?.includes("Workbench fixture") === true);
    const confirmed = await getFaceReviewPage(0, 200);
    if (confirmed.items.filter((item) => item.state === "confirmed").length !== page.items.length) throw new Error("Batch identity assignment did not persist all selected faces");
    const stats = await getMediaResourceStats();
    if (!stats || stats.peakEntries > stats.maxEntries || stats.peakEncodedBytes > stats.maxEncodedBytes) throw new Error("Resource budget exceeded");
    perfMark("faces:workbench-verified", { faces: page.items.length, peakEntries: stats.peakEntries, peakEncodedBytes: stats.peakEncodedBytes });
  } finally { root.unmount(); client.clear(); host.remove(); }
  perfMark("resource:stress-complete");
}
