import { execFileSync } from "node:child_process";

const mode = process.argv[2];
if (mode !== "generate" && mode !== "check") {
  throw new Error("Use contracts.mjs generate or check");
}
execFileSync("cargo", [
  "run", "--locked", "-p", "oxy-core", "--features", "contracts",
  "--bin", "export-contracts", "--", ...(mode === "check" ? ["--check"] : []),
], { stdio: "inherit" });
