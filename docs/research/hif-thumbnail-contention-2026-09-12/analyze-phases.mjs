import fs from "node:fs";

const [resultsPath, logPath, outputPath] = process.argv.slice(2);
const requests = JSON.parse(fs.readFileSync(resultsPath, "utf8"));
const phases = fs.readFileSync(logPath, "utf8").split(/\r?\n/)
  .filter((line) => line.startsWith("HIF_PROFILE heif "))
  .map((line) => Object.fromEntries([...line.matchAll(/(\w+)=([\d.e+-]+)/g)]
    .map((match) => [match[1], Number(match[2])])));
function stats(values) {
  const sorted = values.toSorted((a, b) => a - b);
  return { count: sorted.length, median: sorted[Math.floor(sorted.length / 2)],
    mean: sorted.reduce((a, b) => a + b, 0) / sorted.length,
    min: sorted[0], max: sorted.at(-1) };
}
const summary = {};
for (const key of Object.keys(phases[0])) summary[key] = stats(phases.slice(1).map((row) => row[key]));
for (const key of Object.keys(requests[0])) summary[key] = stats(requests.slice(1).map((row) => row[key]));
const output = {
  method: "Release, 31 distinct fixture paths from the same HIF sample, empty native artifact cache, constructor scans disabled, no Tauri/SQLite/WebView. Each call waits for its async persistence after timing display publication. Summary excludes first publisher initialization; all samples retained. No cold-OS-cache claim.",
  summary, phases, requests,
};
fs.writeFileSync(outputPath, JSON.stringify(output, null, 2) + "\n");
console.log(JSON.stringify(summary, null, 2));
