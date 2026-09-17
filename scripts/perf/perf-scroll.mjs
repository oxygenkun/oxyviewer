// Real WebView/CDP wheel probe. Attach only to an explicitly supplied test port.
// Usage: node scripts/perf/perf-scroll.mjs PORT OUTPUT [--reload] [--reverse] [--filmstrip]
import fs from "node:fs/promises";

const [port, output, ...flags] = process.argv.slice(2);
if (!/^\d+$/.test(port ?? "") || !output) throw new Error("Expected PORT OUTPUT [--reload] [--reverse]");
let target;
const attachDeadline = Date.now() + 30000;
while (!target && Date.now() < attachDeadline) {
  try {
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    target = targets.find((candidate) => candidate.type === "page" && /tauri\.localhost/.test(candidate.url));
  } catch { /* The isolated test app may still be starting. */ }
  if (!target) await new Promise((resolve) => setTimeout(resolve, 30));
}
if (!target) throw new Error("No OxyViewer WebView at the supplied port");
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => { socket.addEventListener("open", resolve, { once: true }); socket.addEventListener("error", reject, { once: true }); });
let nextId = 0;
const pending = new Map();
let finishTrace;
const traceComplete = new Promise((resolve) => { finishTrace = resolve; });
socket.addEventListener("message", ({ data }) => {
  const message = JSON.parse(data);
  if (message.method === "Tracing.tracingComplete") finishTrace(message.params.stream);
  const request = pending.get(message.id);
  if (request) { pending.delete(message.id); message.error ? request.reject(message.error) : request.resolve(message.result); }
});
const call = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++nextId;
  pending.set(id, { resolve, reject });
  socket.send(JSON.stringify({ id, method, params }));
});
const evaluate = async (expression) => {
  const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
  return result.result.value;
};
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function installProbe() {
  if (window.__oxyScrollProbe) window.__oxyScrollProbe.stop();
  const began = performance.now();
  const frames = [], tasks = [], inputs = [], conversions = [], decoded = [];
  const blobUrls = new Map();
  let stopped = false, last = began, wheelStarted, nativeRequestsBefore;
  const frame = (t) => { if (stopped) return; frames.push({ t, duration: t - last }); last = t; requestAnimationFrame(frame); };
  requestAnimationFrame(frame);
  const observer = new PerformanceObserver((list) => tasks.push(...list.getEntries().map((entry) => ({ t: entry.startTime, duration: entry.duration }))));
  observer.observe({ type: "longtask" });
  const createUrl = URL.createObjectURL, revokeUrl = URL.revokeObjectURL;
  URL.createObjectURL = function (blob) { const url = createUrl.call(this, blob); blobUrls.set(url, blob.size); return url; };
  URL.revokeObjectURL = function (url) { blobUrls.delete(url); return revokeUrl.call(this, url); };
  const decode = HTMLImageElement.prototype.decode;
  HTMLImageElement.prototype.decode = async function () {
    await decode.call(this);
    decoded.push({ t: performance.now(), width: this.naturalWidth, height: this.naturalHeight, blob: this.src.startsWith("blob:") });
  };
  const toBlob = HTMLCanvasElement.prototype.toBlob;
  HTMLCanvasElement.prototype.toBlob = function (...args) {
    const started = performance.now();
    const result = toBlob.apply(this, args);
    conversions.push({ t: started, duration: performance.now() - started, width: this.width, height: this.height });
    return result;
  };
  const scroller = () => document.querySelector(".filmstrip") ?? document.querySelector(".asset-scroll");
  const position = (element) => element.matches(".filmstrip") ? element.scrollLeft : element.scrollTop;
  const wheel = (event) => {
    const element = scroller();
    if (!element?.contains(event.target)) return;
    const horizontal = element.matches(".filmstrip");
    const before = position(element);
    const max = horizontal ? element.scrollWidth - element.clientWidth : element.scrollHeight - element.clientHeight;
    const delta = horizontal ? event.deltaX || event.deltaY : event.deltaY;
    inputs.push({ t: performance.now(), trusted: event.isTrusted, before, deltaX: event.deltaX, deltaY: event.deltaY, deltaMode: event.deltaMode,
      atBoundary: (delta < 0 && before <= 0) || (delta > 0 && before >= max - 1) });
  };
  const scroll = (event) => {
    const element = scroller();
    if (event.target !== element) return;
    const after = position(element);
    const now = performance.now();
    for (let index = inputs.length - 1; index >= 0 && inputs[index].latency === undefined; index--) {
      if (!inputs[index].atBoundary && inputs[index].before !== after) { inputs[index].latency = now - inputs[index].t; inputs[index].after = after; }
    }
  };
  document.addEventListener("wheel", wheel, { capture: true, passive: true });
  document.addEventListener("scroll", scroll, { capture: true, passive: true });
  const distribution = (values) => {
    values.sort((a, b) => a - b);
    return { count: values.length, p50: values[Math.floor(values.length * .5)], p95: values[Math.floor(values.length * .95)], max: values.at(-1) };
  };
  window.__oxyScrollProbe = {
    startWheel(reverse, requireFilmstrip) {
      if (!performance.getEntriesByName("first-contentful-paint").length) return null;
      const element = scroller();
      if (!element || (requireFilmstrip && !element.matches(".filmstrip"))) return null;
      if (reverse) element[element.matches(".filmstrip") ? "scrollLeft" : "scrollTop"] = element.scrollWidth + element.scrollHeight;
      else element[element.matches(".filmstrip") ? "scrollLeft" : "scrollTop"] = 0;
      const rect = element.getBoundingClientRect();
      wheelStarted = performance.now();
      nativeRequestsBefore = window.__oxyPerfInspect?.().marks.filter((mark) => mark.name === "preview:queued").length;
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    },
    stop() {
      stopped = true;
      observer.disconnect();
      document.removeEventListener("wheel", wheel, true);
      document.removeEventListener("scroll", scroll, true);
      URL.createObjectURL = createUrl; URL.revokeObjectURL = revokeUrl;
      HTMLImageElement.prototype.decode = decode; HTMLCanvasElement.prototype.toBlob = toBlob;
      const element = scroller();
      const rect = element?.getBoundingClientRect();
      const visibleImages = [...(element?.querySelectorAll("img") ?? [])].filter((image) => {
        const bounds = image.getBoundingClientRect();
        return bounds.right > rect.left && bounds.left < rect.right && bounds.bottom > rect.top && bounds.top < rect.bottom;
      }).map((image) => ({ width: image.naturalWidth, height: image.naturalHeight, ready: image.complete && image.naturalWidth > 0, source: image.src.startsWith("blob:") ? "blob" : "native" }));
      const duringWheel = tasks.filter((entry) => entry.t >= wheelStarted);
      return { began, wheelStarted, ended: performance.now(), surface: element?.matches(".filmstrip") ? "filmstrip" : "grid",
        paint: performance.getEntriesByType("paint").map((entry) => ({ name: entry.name, t: entry.startTime })),
        nativeRequestsBefore,
        frameIntervals: frames,
        visibility: document.visibilityState, focused: document.hasFocus(), viewport: [innerWidth, innerHeight], dpr: devicePixelRatio,
        startupFrames: distribution(frames.filter((entry) => entry.t - entry.duration < wheelStarted).map((entry) => entry.duration)),
        frames: distribution(frames.filter((entry) => entry.t - entry.duration >= wheelStarted).map((entry) => entry.duration)),
        startupTasks: tasks.filter((entry) => entry.t < wheelStarted), tasks: duringWheel,
        longTaskTotalMs: duringWheel.reduce((sum, entry) => sum + entry.duration, 0),
        inputs, inputLatency: distribution(inputs.flatMap((entry) => entry.latency === undefined ? [] : [entry.latency])),
        conversions, decoded, liveBlobs: blobUrls.size, liveBlobBytes: [...blobUrls.values()].reduce((a, b) => a + b, 0),
        endPosition: element ? position(element) : null, visibleImages };
    },
  };
}

let injected;
try {
  await call("Page.enable");
  if (flags.includes("--trace")) await call("Tracing.start", {
    categories: "toplevel,cc,gpu,viz,blink,devtools.timeline,disabled-by-default-devtools.timeline",
    transferMode: "ReturnAsStream",
  });
  injected = (await call("Page.addScriptToEvaluateOnNewDocument", { source: `(${installProbe.toString()})()` })).identifier;
  await call("Page.bringToFront");
  const expected = Number(flags.find((flag) => flag.startsWith("--await-retained="))?.split("=")[1] ?? 0);
  if (expected) {
    const deadline = Date.now() + 150000;
    while (await evaluate("window.__oxyPerfInspect?.().retained.count ?? 0") < expected) {
      if (Date.now() > deadline) throw new Error("Folder thumbnail warming timed out");
      await pause(100);
    }
  }
  if (flags.includes("--reload")) {
    await call("Page.reload", { ignoreCache: false });
  } else { await evaluate(`(${installProbe.toString()})()`); }
  const deadline = Date.now() + 30000;
  let point;
  while (!point && Date.now() < deadline) {
    point = await evaluate(`window.__oxyScrollProbe?.startWheel(${flags.includes("--reverse")}, ${flags.includes("--filmstrip")}) ?? null`);
    if (!point) await pause(30);
  }
  if (!point) throw new Error("No scroll surface appeared within 30 seconds");
  const inputs = [];
  const steps = Number(flags.find((flag) => flag.startsWith("--steps="))?.split("=")[1] ?? 150);
  if (!Number.isInteger(steps) || steps < 1 || steps > 2000) throw new Error("Invalid --steps value");
  for (let step = 0; step < steps; step++) {
    inputs.push(call("Input.dispatchMouseEvent", { type: "mouseWheel", ...point, deltaX: 0, deltaY: flags.includes("--reverse") ? -120 : 120 }));
    await pause(30);
  }
  await Promise.all(inputs);
  await pause(900);
  const report = await evaluate("window.__oxyScrollProbe.stop()");
  report.browser = await evaluate("window.__oxyPerfInspect?.() ?? null");
  report.nativeRequestsOnWheel = report.browser && report.nativeRequestsBefore !== undefined
    ? report.browser.marks.filter((mark) => mark.name === "preview:queued").length - report.nativeRequestsBefore : null;
  report.loupeImages = await evaluate(`Array.from(document.querySelectorAll('.loupe__render img')).map(image => ({
    width: image.naturalWidth, height: image.naturalHeight, ready: image.complete,
    pending: image.classList.contains('thumbnail__pending-image'), decoding: image.decoding,
    opacity: getComputedStyle(image).opacity, visibility: getComputedStyle(image).visibility }))`);
  report.native = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_media_resource_stats').catch(error => ({ error: String(error) }))`);
  report.queue = await evaluate(`window.__TAURI_INTERNALS__.invoke('get_debug_queue_snapshot').catch(error => ({ error: String(error) }))`);
  const screenshot = await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await fs.writeFile(output.replace(/\.json$/, "") + ".png", Buffer.from(screenshot.data, "base64"));
  if (flags.includes("--trace")) {
    await call("Tracing.end");
    const handle = await traceComplete;
    const chunks = [];
    for (;;) {
      const chunk = await call("IO.read", { handle });
      chunks.push(chunk.base64Encoded ? Buffer.from(chunk.data, "base64").toString("utf8") : chunk.data);
      if (chunk.eof) break;
    }
    await call("IO.close", { handle });
    await fs.writeFile(output.replace(/\.json$/, "") + ".trace.json", chunks.join(""));
  }
  await fs.writeFile(output, JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ surface: report.surface, frames: report.frames, tasks: report.tasks,
    startupFrames: report.startupFrames, paint: report.paint, retained: report.browser?.retained,
    nativeRequestsOnWheel: report.nativeRequestsOnWheel,
    inputLatency: report.inputLatency, conversions: report.conversions.length, decoded: report.decoded.length,
    liveBlobs: report.liveBlobs, visibleCount: report.visibleImages.length,
    visibleReady: report.visibleImages.filter((image) => image.ready).length, loupeImages: report.loupeImages,
    focused: report.focused }));
} finally {
  if (injected) await call("Page.removeScriptToEvaluateOnNewDocument", { identifier: injected });
  socket.close();
}
