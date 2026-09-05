// Bounded diagnostics from the real IPC/render path, including normal startup.
// Inspect with window.__oxyBrowseDiagnostics in the desktop WebView console.
export interface BrowseTiming {
  phase: string;
  atMs: number;
  [key: string]: unknown;
}
const timings: BrowseTiming[] = [];
export function recordBrowseTiming(phase: string, detail: Record<string, unknown> = {}): void {
  const previous = timings.at(-1);
  if (phase === "native-browse" && previous?.phase === phase &&
    previous.sessionId === detail.sessionId && previous.directory === detail.directory &&
    previous.stage === detail.stage && previous.source === detail.source) {
    timings[timings.length - 1] = { ...detail, phase, atMs: performance.now(), startedAtMs: previous.startedAtMs ?? previous.atMs };
    return;
  }
  timings.push({ ...detail, phase, atMs: performance.now() });
  if (timings.length > 200) timings.splice(0, timings.length - 200);
}
if (typeof window !== "undefined") {
  Object.defineProperty(window, "__oxyBrowseDiagnostics", {
    configurable: true,
    get: () => timings.map((timing) => ({ ...timing })),
  });
}
