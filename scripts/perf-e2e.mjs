#!/usr/bin/env node
/**
 * End-to-end performance regression runner. Launches the packaged release app
 * with an injected scenario (OXY_PERF_SCENARIO), lets the in-app performance
 * harness drive the real UI/backend pipeline, collects JSON reports, and
 * checks absolute budgets plus baseline regressions. See docs/PERF_E2E.md.
 *
 * Usage:
 *   node scripts/perf-e2e.mjs [--scenario <name>]... [--runs N]
 *                             [--update-baseline] [--verbose] [--app <path>]
 *   node scripts/perf-e2e.mjs --folder <path> --select-name <file>
 *                             [--scroll-end] [--cold-cache] [--await-mark <token>]
 *
 * Requires an embedded release build: pnpm tauri build --no-bundle
 */
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const PERF_DIR = path.join(ROOT, "tests", "perf");
const GENERATED_DIR = path.join(PERF_DIR, "generated");
const REPORTS_DIR = path.join(PERF_DIR, ".reports");
const SCENARIOS_PATH = path.join(PERF_DIR, "scenarios.json");
const BASELINE_PATH = path.join(PERF_DIR, "baseline.json");

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const args = {
    scenarios: [],
    runs: undefined,
    updateBaseline: false,
    verbose: false,
    app: undefined,
    folder: undefined,
    selectName: undefined,
    scrollEnd: false,
    coldCache: false,
    awaitMarks: [],
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--scenario") args.scenarios.push(argv[++index]);
    else if (arg === "--runs") args.runs = Number(argv[++index]);
    else if (arg === "--update-baseline") args.updateBaseline = true;
    else if (arg === "--verbose") args.verbose = true;
    else if (arg === "--app") args.app = argv[++index];
    else if (arg === "--folder") args.folder = argv[++index];
    else if (arg === "--select-name") args.selectName = argv[++index];
    else if (arg === "--scroll-end") args.scrollEnd = true;
    else if (arg === "--cold-cache") args.coldCache = true;
    else if (arg === "--await-mark") args.awaitMarks.push(argv[++index]);
    else if (arg === "--help" || arg === "-h") {
      console.log("Usage: node scripts/perf-e2e.mjs [--scenario name]... [--runs N] [--update-baseline] [--verbose] [--app path]");
      console.log("       node scripts/perf-e2e.mjs --folder path --select-name file [--scroll-end] [--cold-cache] [--await-mark token]");
      process.exit(0);
    } else {
      console.error(`Unknown argument: ${arg}`);
      process.exit(2);
    }
  }
  return args;
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/** Builds a solid-color RGB PNG without external dependencies. */
function makeSolidPng(width, height, [r, g, b]) {
  const stride = 1 + width * 3;
  const raw = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y += 1) {
    const row = y * stride;
    raw[row] = 0; // filter: none
    for (let x = 0; x < width; x += 1) {
      const px = row + 1 + x * 3;
      raw[px] = (r + x) % 256;
      raw[px + 1] = (g + y) % 256;
      raw[px + 2] = b;
    }
  }
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length, 0);
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(zlib.crc32(body), 0);
    return Buffer.concat([length, body, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // color type: truecolor RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function linkOrCopy(source, destination) {
  try {
    fs.linkSync(source, destination);
  } catch {
    fs.copyFileSync(source, destination);
  }
}

/** Creates (or reuses) a directory with `count` synthetic images. */
function prepareSyntheticFixture(name, fixture) {
  const { format, count, width, height } = fixture;
  const extension = format === "jpeg" ? "jpg" : "png";
  const directory = path.join(GENERATED_DIR, `${name}-${count}`);
  const markerPath = path.join(directory, ".marker.json");
  const marker = JSON.stringify({ format, count, width, height });
  if (fs.existsSync(markerPath) && fs.readFileSync(markerPath, "utf8") === marker) {
    return directory;
  }
  fs.rmSync(directory, { recursive: true, force: true });
  fs.mkdirSync(directory, { recursive: true });
  const png = makeSolidPng(width, height, [128, 64, 200]);
  const seedPath = path.join(directory, `IMG_000001.${extension}`);
  if (format === "jpeg") {
    const pngPath = path.join(directory, ".seed.png");
    fs.writeFileSync(pngPath, png);
    try {
      execFileSync("sips", ["-s", "format", "jpeg", pngPath, "--out", seedPath], { stdio: "pipe" });
    } catch {
      throw new Error(
        `Scenario "${name}" needs JPEG fixtures; generating them requires macOS 'sips'. ` +
        "Provide a 'file' fixture instead or run on macOS.",
      );
    } finally {
      fs.rmSync(pngPath, { force: true });
    }
  } else {
    fs.writeFileSync(seedPath, png);
  }
  for (let index = 2; index <= count; index += 1) {
    linkOrCopy(seedPath, path.join(directory, `IMG_${String(index).padStart(6, "0")}.${extension}`));
  }
  fs.writeFileSync(markerPath, marker);
  return directory;
}

/** Creates (or reuses) a single-file directory holding the fixture file. */
function prepareFileFixture(name, fixture) {
  const source = path.resolve(ROOT, fixture.path);
  if (!fs.existsSync(source)) throw new Error(`Fixture not found: ${source}`);
  const directory = path.join(GENERATED_DIR, name);
  const destination = path.join(directory, path.basename(source));
  const markerPath = path.join(directory, ".marker.json");
  const marker = JSON.stringify({ source, mtimeMs: fs.statSync(source).mtimeMs });
  if (fs.existsSync(markerPath) && fs.readFileSync(markerPath, "utf8") === marker) {
    return directory;
  }
  fs.rmSync(directory, { recursive: true, force: true });
  fs.mkdirSync(directory, { recursive: true });
  linkOrCopy(source, destination);
  fs.writeFileSync(markerPath, marker);
  return directory;
}

function prepareFixture(name, fixture) {
  if (fixture.type === "directory") {
    const directory = path.resolve(fixture.path);
    if (!fs.statSync(directory, { throwIfNoEntry: false })?.isDirectory()) {
      throw new Error(`Fixture directory not found: ${directory}`);
    }
    return directory;
  }
  fs.mkdirSync(GENERATED_DIR, { recursive: true });
  return fixture.type === "synthetic"
    ? prepareSyntheticFixture(name, fixture)
    : prepareFileFixture(name, fixture);
}

// ---------------------------------------------------------------------------
// Runner-owned application state (never the normal user cache/database)
// ---------------------------------------------------------------------------

function hasManagedArtifact(cacheDir) {
  const root = path.join(cacheDir, "media-cache-v2");
  if (!fs.statSync(root, { throwIfNoEntry: false })?.isDirectory()) return false;
  for (const prefix of fs.readdirSync(root, { withFileTypes: true })) {
    if (!prefix.isDirectory() || !/^[0-9a-f]{2}$/.test(prefix.name)) continue;
    const prefixPath = path.join(root, prefix.name);
    for (const source of fs.readdirSync(prefixPath, { withFileTypes: true })) {
      if (!source.isDirectory() || !/^[0-9a-f]{64}$/.test(source.name)) continue;
      const hasArtifact = fs
        .readdirSync(path.join(prefixPath, source.name), { withFileTypes: true })
        .some((entry) => entry.isFile() && entry.name.endsWith(".jpg"));
      if (hasArtifact) return true;
    }
  }
  return false;
}

function clearIsolatedState(runtime, clearData = false) {
  fs.rmSync(runtime.cacheDir, { recursive: true, force: true });
  if (clearData) fs.rmSync(runtime.dataDir, { recursive: true, force: true });
}

// ---------------------------------------------------------------------------
// App process
// ---------------------------------------------------------------------------

function defaultAppBinary() {
  const name = process.platform === "win32" ? "oxyviewer.exe" : "oxyviewer";
  return path.join(ROOT, "target", "release", name);
}

function runScenarioOnce(
  appBinary,
  scenarioPayload,
  runtime,
  timeoutMs,
  verbose,
  settleAfterReportMs = 0,
) {
  return new Promise((resolve) => {
    fs.rmSync(scenarioPayload.reportPath, { force: true });
    const child = spawn(appBinary, [], {
      env: {
        ...process.env,
        OXY_PERF_SCENARIO: JSON.stringify(scenarioPayload),
        OXY_PERF_DATA_DIR: runtime.dataDir,
        OXY_PERF_CACHE_DIR: runtime.cacheDir,
      },
      stdio: ["ignore", "pipe", "pipe"],
      detached: process.platform !== "win32",
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
      if (stderr.length > 64_000) stderr = stderr.slice(-64_000);
    });
    if (verbose) child.stdout.on("data", (chunk) => process.stdout.write(`[app] ${chunk}`));

    const kill = () => {
      try {
        if (process.platform === "win32") child.kill("SIGKILL");
        else process.kill(-child.pid, "SIGKILL");
      } catch { /* already exited */ }
    };
    const deadline = Date.now() + timeoutMs + 30_000;
    const poll = setInterval(() => {
      if (fs.existsSync(scenarioPayload.reportPath)) {
        clearInterval(poll);
        const finish = () => {
          kill();
          try {
            const report = JSON.parse(fs.readFileSync(scenarioPayload.reportPath, "utf8"));
            resolve({ ok: true, report, stderr });
          } catch (error) {
            resolve({ ok: false, error: `invalid report: ${error}`, stderr });
          }
        };
        if (settleAfterReportMs > 0) setTimeout(finish, settleAfterReportMs);
        else finish();
      } else if (Date.now() > deadline) {
        clearInterval(poll);
        kill();
        resolve({ ok: false, error: "runner deadline exceeded (no report written)", stderr });
      }
    }, 100);
    child.on("error", (error) => {
      clearInterval(poll);
      resolve({ ok: false, error: `failed to launch app: ${error}`, stderr });
    });
  });
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

function markTime(report, name, detailKey, detailValue) {
  const marks = report.marks.filter((entry) =>
    entry.name === name
    && (detailKey === undefined || entry.detail?.[detailKey] === detailValue)
  );
  return marks.length > 0 ? Math.min(...marks.map((entry) => entry.t)) : undefined;
}

function computeMetrics(report, selectName) {
  const metrics = {};
  const openRequested = markTime(report, "folder:open-requested");
  const firstPainted = markTime(report, "harness:first-page-painted");
  if (openRequested !== undefined && firstPainted !== undefined) {
    metrics.firstPageMs = firstPainted - openRequested;
  }
  const firstPageReturned = report.marks.find((mark) => mark.name === "assets:first-page-returned");
  if (firstPageReturned) {
    const returnedAt = firstPageReturned.t;
    if (openRequested !== undefined) metrics.firstPageReturnMs = returnedAt - openRequested;
    if (firstPainted !== undefined) metrics.firstPagePaintMs = firstPainted - returnedAt;
    for (const [detail, metric] of [
      ["cacheMs", "browse:cacheMs"],
      ["resolveMs", "browse:resolveMs"],
      ["enumerationMs", "browse:enumerationMs"],
      ["attributesMs", "browse:attributesMs"],
      ["snapshotSerializeMs", "browse:snapshotSerializeMs"],
      ["snapshotPersistMs", "browse:snapshotPersistMs"],
      ["sortMs", "browse:sortMs"],
      ["elapsedMs", "browse:nativeMs"],
      ["ipcMs", "browse:ipcMs"],
    ]) {
      if (firstPageReturned.detail?.[detail] !== undefined) {
        metrics[metric] = firstPageReturned.detail[detail];
      }
    }
  }
  const select = markTime(report, "harness:select");
  if (select !== undefined && selectName) {
    const loaded = report.marks.filter((mark) =>
      mark.name === "image:loaded"
      && mark.detail?.assetName === selectName
      && mark.detail?.large === true
    );
    if (loaded.length > 0) {
      metrics.firstPreviewMs = Math.min(...loaded.map((mark) => mark.t)) - select;
    }
    for (const [level, metric] of [["thumbnail", "thumbnailMs"], ["preview", "previewMs"], ["full", "fullMs"]]) {
      const mark = report.marks.find((entry) =>
        entry.name === "image:loaded"
        && entry.detail?.stage === level
        && entry.detail?.assetName === selectName
      );
      if (mark) metrics[metric] = mark.t - select;
    }
    const firstTile = markTime(report, "heif:first-tile-painted", "assetName", selectName);
    if (firstTile !== undefined) metrics.heifFirstTileMs = firstTile - select;
    const allTiles = markTime(report, "heif:all-tiles-painted", "assetName", selectName);
    if (allTiles !== undefined) metrics.heifAllTilesMs = allTiles - select;
    const fullCacheHit = markTime(report, "heif:full-cache-hit", "assetName", selectName);
    if (fullCacheHit !== undefined) metrics.heifFullCacheHitMs = fullCacheHit - select;
    // Backend arrival of the semantic preview level, independent of which
    // concrete dimensions the platform/format policy selected.
    const previewResult = report.marks.find((mark) =>
      mark.name === "preview:result"
      && mark.detail?.assetName === selectName
      && mark.detail?.level === "preview"
    );
    if (previewResult) metrics.previewResultMs = previewResult.t - select;
  }
  const viewportJump = markTime(report, "harness:viewport-jump");
  if (viewportJump !== undefined && selectName) {
    const loaded = report.marks.find((mark) =>
      mark.name === "image:loaded"
      && mark.detail?.assetName === selectName
      && mark.detail?.large === false
    );
    if (loaded) metrics.viewportJumpPreviewMs = loaded.t - viewportJump;
  }
  // Backend-reported durations (recorded only, no absolute budgets).
  for (const mark of report.marks) {
    if (mark.name === "preview:result" && mark.detail?.diagnostics?.totalMs !== undefined) {
      metrics[`backend:${String(mark.detail.level)}Ms`] =
        mark.detail.diagnostics.totalMs;
    }
    if (mark.name === "heif:backend-complete" && mark.detail?.diagnostics?.totalMs !== undefined) {
      metrics["backend:heifSessionMs"] = mark.detail.diagnostics.totalMs;
    }
  }
  const done = report.marks.find((mark) => mark.name === "harness:done");
  metrics.completed = done?.detail?.reason === "complete" ? 1 : 0;
  return metrics;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function p95(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * 0.95) - 1)];
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const config = JSON.parse(fs.readFileSync(SCENARIOS_PATH, "utf8"));
  if (args.folder) {
    if (args.scrollEnd && !args.selectName) {
      throw new Error("--scroll-end requires --select-name so the painted target can be verified");
    }
    config.scenarios["manual-folder"] = {
      fixture: { type: "directory", path: args.folder },
      selectName: args.selectName,
      enterLoupe: !args.scrollEnd,
      scrollToEnd: args.scrollEnd,
      awaitMarks: args.awaitMarks.length > 0
        ? args.awaitMarks
        : args.selectName
          ? ["image:loaded"]
          : ["harness:first-page-painted"],
      coldCache: args.coldCache,
      runs: 1,
      timeoutMs: 60_000,
      budgets: args.scrollEnd
        ? { viewportJumpPreviewMs: 800 }
        : args.selectName
          ? { firstPreviewMs: 800 }
          : {},
    };
  }
  const baseline = fs.existsSync(BASELINE_PATH)
    ? JSON.parse(fs.readFileSync(BASELINE_PATH, "utf8"))
    : { scenarios: {} };
  const regressionFactor = config.regressionFactor ?? 1.2;
  const appBinary = args.app ?? defaultAppBinary();
  if (!fs.existsSync(appBinary)) {
    console.error(`Release binary not found: ${appBinary}`);
    console.error("Build it first: pnpm tauri build --no-bundle");
    process.exit(2);
  }
  fs.mkdirSync(REPORTS_DIR, { recursive: true });

  const names = args.folder
    ? ["manual-folder"]
    : args.scenarios.length > 0
      ? args.scenarios
      : Object.keys(config.scenarios);
  const failures = [];
  const newBaseline = { meta: baseline.meta ?? null, scenarios: { ...baseline.scenarios } };

  for (const name of names) {
    const scenario = config.scenarios[name];
    if (!scenario) {
      console.error(`Unknown scenario: ${name}`);
      failures.push(`${name}: unknown scenario`);
      continue;
    }
    let folder;
    try {
      folder = prepareFixture(name, scenario.fixture);
    } catch (error) {
      console.error(`\n== ${name} == SKIPPED: ${error.message}`);
      continue;
    }
    const runs = args.runs ?? scenario.runs ?? 3;
    const runtime = {
      dataDir: path.join(REPORTS_DIR, ".runtime", name, "data"),
      cacheDir: path.join(REPORTS_DIR, ".runtime", name, "cache", "previews"),
    };
    fs.rmSync(path.join(REPORTS_DIR, ".runtime", name), { recursive: true, force: true });
    const selectName = scenario.selectName
      ?? (scenario.fixture.type === "file" ? path.basename(scenario.fixture.path) : undefined);
    console.log(`\n== ${name} == runs=${runs} coldCache=${Boolean(scenario.coldCache)} folder=${folder}`);

    const payload = (reportName, awaitMarks = scenario.awaitMarks) => ({
      name,
      folder,
      selectName,
      enterLoupe: scenario.enterLoupe,
      scrollToEnd: scenario.scrollToEnd,
      awaitMarks: awaitMarks ?? ["harness:first-page-painted"],
      timeoutMs: scenario.timeoutMs,
      reportPath: path.join(REPORTS_DIR, reportName),
    });

    if (scenario.warmup && !scenario.coldCache) {
      if (args.verbose) console.log("  warmup run (not measured)");
      if (scenario.clearCacheBeforeWarmup) clearIsolatedState(runtime, true);
      const warmup = await runScenarioOnce(
        appBinary,
        payload(`${name}.warmup.json`, scenario.warmupAwaitMarks),
        runtime,
        scenario.timeoutMs ?? 30_000,
        args.verbose,
        scenario.warmupSettleMs ?? 0,
      );
      if (!warmup.ok) {
        const message = `${name}: warmup failed: ${warmup.error}`;
        failures.push(message);
        console.error(`  WARMUP FAILED: ${warmup.error}; measured runs rejected`);
        if (warmup.stderr) console.error(`  stderr tail: ${warmup.stderr.slice(-2000)}`);
        continue;
      }
      const warmupMetrics = computeMetrics(warmup.report, selectName);
      if (!warmupMetrics.completed) {
        failures.push(`${name}: warmup timed out or required marks were missing`);
        console.error("  WARMUP INCOMPLETE: measured runs rejected");
        continue;
      }
      if (scenario.requireWarmArtifact
        && !hasManagedArtifact(runtime.cacheDir)) {
        failures.push(`${name}: warmup did not publish a managed cache artifact`);
        console.error("  WARMUP INCOMPLETE: no managed cache artifact; measured runs rejected");
        continue;
      }
    }

    const samples = [];
    for (let run = 0; run < runs; run += 1) {
      if (scenario.coldCache) clearIsolatedState(runtime, true);
      const reportName = `${name}.run${run + 1}.json`;
      const result = await runScenarioOnce(
        appBinary,
        payload(reportName),
        runtime,
        scenario.timeoutMs ?? 30_000,
        args.verbose,
      );
      if (!result.ok) {
        console.error(`  run ${run + 1}: FAILED to produce report: ${result.error}`);
        if (result.stderr) console.error(`  stderr tail: ${result.stderr.slice(-2000)}`);
        failures.push(`${name} run ${run + 1}: ${result.error}`);
        continue;
      }
      if (scenario.expectedHeifBackend) {
        const backend = result.report.marks.find((mark) =>
          mark.name === "heif:backend-complete"
          && mark.detail?.assetName === selectName
        )?.detail?.diagnostics?.backend;
        if (backend !== scenario.expectedHeifBackend) {
          const error = `expected HEIF backend ${scenario.expectedHeifBackend}, received ${backend ?? "none"}`;
          console.error(`  run ${run + 1}: REJECTED: ${error}`);
          failures.push(`${name} run ${run + 1}: ${error}`);
          continue;
        }
      }
      if (scenario.expectedPreviewBackend) {
        const backend = result.report.marks.find((mark) =>
          mark.name === "preview:result"
          && mark.detail?.assetName === selectName
          && mark.detail?.diagnostics?.backend
        )?.detail?.diagnostics?.backend;
        if (backend !== scenario.expectedPreviewBackend) {
          const error = `expected preview backend ${scenario.expectedPreviewBackend}, received ${backend ?? "none"}`;
          console.error(`  run ${run + 1}: REJECTED: ${error}`);
          failures.push(`${name} run ${run + 1}: ${error}`);
          continue;
        }
      }
      const metrics = computeMetrics(result.report, selectName);
      samples.push(metrics);
      if (args.verbose) {
        console.log(`  run ${run + 1}: ${JSON.stringify(metrics)}`);
      } else {
        console.log(`  run ${run + 1}: ${metrics.completed ? "complete" : "INCOMPLETE"}`);
      }
      if (!metrics.completed) failures.push(`${name} run ${run + 1}: scenario timed out or marks missing`);
    }
    if (samples.length === 0) continue;

    // Aggregate numeric metrics.
    const keys = [...new Set(samples.flatMap((sample) =>
      Object.keys(sample).filter((key) => key !== "completed" && typeof sample[key] === "number")
    ))];
    const summary = {};
    for (const key of keys) {
      const values = samples.flatMap((sample) => (typeof sample[key] === "number" ? [sample[key]] : []));
      summary[key] = { median: median(values), p95: p95(values), min: Math.min(...values), n: values.length };
    }

    console.log("  metrics (median / p95, ms):");
    for (const [key, stats] of Object.entries(summary)) {
      console.log(`    ${key}: ${stats.median.toFixed(1)} / ${stats.p95.toFixed(1)}`);
    }

    // Absolute budget checks.
    for (const [metric, budget] of Object.entries(scenario.budgets ?? {})) {
      const stats = summary[metric];
      if (!stats) {
        failures.push(`${name}: budgeted metric ${metric} was not recorded`);
        console.error(`    BUDGET MISS: ${metric} not recorded (budget ${budget} ms)`);
        continue;
      }
      if (stats.median > budget) {
        failures.push(`${name}: ${metric} median ${stats.median.toFixed(1)} ms > budget ${budget} ms`);
        console.error(`    BUDGET FAIL: ${metric} median ${stats.median.toFixed(1)} ms > ${budget} ms`);
      } else {
        console.log(`    budget ok: ${metric} ${stats.median.toFixed(1)} ms <= ${budget} ms`);
      }
    }

    // Baseline regression checks.
    const baselineEntry = baseline.scenarios?.[name] ?? {};
    for (const [key, stats] of Object.entries(summary)) {
      const reference = baselineEntry[key]?.median;
      if (reference === undefined) {
        if (!args.updateBaseline) console.log(`    baseline: no reference for ${key} (record-only)`);
        continue;
      }
      const limit = reference * regressionFactor;
      if (stats.median > limit) {
        failures.push(
          `${name}: ${key} median ${stats.median.toFixed(1)} ms regressed vs baseline ${reference.toFixed(1)} ms (limit ${limit.toFixed(1)} ms)`,
        );
        console.error(`    REGRESSION: ${key} ${stats.median.toFixed(1)} ms > ${limit.toFixed(1)} ms (baseline ${reference.toFixed(1)} ms)`);
      }
    }

    if (args.updateBaseline) {
      newBaseline.scenarios[name] = Object.fromEntries(
        Object.entries(summary).map(([key, stats]) => [key, { median: stats.median, p95: stats.p95 }]),
      );
    }
  }

  if (args.updateBaseline) {
    newBaseline.meta = {
      updatedAt: new Date().toISOString(),
      platform: process.platform,
      arch: process.arch,
      host: os.hostname(),
      regressionFactor,
    };
    fs.writeFileSync(BASELINE_PATH, `${JSON.stringify(newBaseline, null, 2)}\n`);
    console.log(`\nBaseline updated: ${path.relative(ROOT, BASELINE_PATH)}`);
  }

  console.log(`\nReports written to ${path.relative(ROOT, REPORTS_DIR)}/`);
  if (failures.length > 0) {
    console.error(`\nFAILED (${failures.length}):`);
    for (const failure of failures) console.error(`  - ${failure}`);
    process.exit(1);
  }
  console.log("\nAll scenarios passed.");
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
