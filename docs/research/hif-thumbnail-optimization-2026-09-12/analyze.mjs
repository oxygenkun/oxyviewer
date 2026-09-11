// Usage: node analyze.mjs report1.json [report2.json ...] output.json
// Keep numerical evidence and public fixture information, not local paths.
import fs from "node:fs";

const inputs = process.argv.slice(2, -1);
const output = process.argv.at(-1);
if (!inputs.length || !output) throw new Error("Expected reports and an output path");
const round = (value) => Math.round(value * 100) / 100;
function stats(values) {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (!sorted.length) return { count: 0 };
  const at = (fraction) => sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)];
  const median = (sorted[Math.floor((sorted.length - 1) / 2)] + sorted[Math.floor(sorted.length / 2)]) / 2;
  return { count: sorted.length, median: round(median), p95: round(at(0.95)), max: round(sorted.at(-1)) };
}
const runs = inputs.map((path) => {
  const report = JSON.parse(fs.readFileSync(path, "utf8"));
  const marks = report.marks;
  const first = (name) => marks.find((mark) => mark.name === name);
  const warming = first("folder-thumbnails:warmed");
  const complete = first("resource:stress-complete");
  const queued = marks.filter((mark) => mark.name === "folder-thumbnails:queue-snapshot");
  const thumbnails = marks.filter((mark) => mark.name === "preview:result" && mark.detail.level === "thumbnail");
  const firstPage = first("harness:first-page-painted");
  const opened = first("folder:open-requested");
  return {
    scenario: report.scenario,
    generatedAt: report.generatedAt,
    complete: !!complete,
    firstPageMs: firstPage && opened ? round(firstPage.t - opened.t) : null,
    warmingMs: warming ? round(warming.detail.elapsedMs) : null,
    retainedCount: warming?.detail.count ?? null,
    decodedBytes: warming?.detail.decodedBytes ?? null,
    viewportReadinessMs: complete?.detail.viewportWaitsMs.map(round) ?? [],
    nativeRequestsOnWarmScroll: complete?.detail.nativeRequestsOnWarmScroll ?? null,
    nativeResources: complete?.detail.native ?? null,
    last20ThumbnailNativeTotalMs: stats(thumbnails.slice(-20).map((mark) => mark.detail.diagnostics?.totalMs)),
    queueRoundTripMs: stats(queued.map((mark) => mark.detail.elapsedMs)),
    queueNativeCollectionMicros: stats(queued.map((mark) => mark.detail.collectionMicros)),
    queueNativeWorkerWaitMicros: stats(queued.map((mark) => mark.detail.workerWaitMicros)),
    queueRoundTripAfterFirstSecondMs: stats(queued.filter((mark) => mark.t >= 1000).map((mark) => mark.detail.elapsedMs)),
    queueErrors: marks.filter((mark) => mark.name === "folder-thumbnails:queue-snapshot-error").length,
    queueSamples: queued.map((mark) => ({ t: round(mark.t), ...mark.detail, elapsedMs: round(mark.detail.elapsedMs) })),
  };
});
const result = {
  date: "2026-09-12",
  platform: "Windows Release / WebView2",
  fixture: "tests/fixtures/DSC00449.HIF, 1100 hardlink-or-copy paths",
  conditions: "Each run uses cold isolated application state and cache; OS file cache is not cleared. No concurrent build during these runs.",
  limits: "Viewport checks include at least 40 ms settling time. Decoded bytes exclude Blob and browser overhead. This is one photo at distinct paths, not a diverse-photo or NAS benchmark.",
  runs,
};
fs.writeFileSync(output, `${JSON.stringify(result, null, 2)}\n`);
console.log(JSON.stringify(runs.map(({ queueSamples, ...summary }) => summary), null, 2));
