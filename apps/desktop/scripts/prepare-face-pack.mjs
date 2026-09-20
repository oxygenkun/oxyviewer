import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const workspace = fileURLToPath(new URL("../../../", import.meta.url));

export function prepareFacePack(target, release = true) {
  const args = ["build", "-p", "oxy-analyzer-host", "--bin", "oxy-face-worker"];
  if (release) args.push("--release");
  if (target) args.push("--target", target);
  const build = spawnSync("cargo", args, { cwd: workspace, stdio: "inherit" });
  if (build.status !== 0) throw new Error("Face worker build failed");
  const rustHost = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
  const triple = target ?? rustHost.stdout.match(/^host: (.+)$/m)?.[1];
  if (!triple) throw new Error("Cannot determine face worker target");
  const targetOs = triple.includes("windows") ? "windows" : triple.includes("apple") ? "macos" : "linux";
  const targetArch = triple.startsWith("aarch64") ? "aarch64" : triple.startsWith("x86_64") ? "x86_64" : null;
  if (!targetArch) throw new Error(`Unsupported face worker target: ${triple}`);
  const name = `oxy-face-worker${targetOs === "windows" ? ".exe" : ""}`;
  const source = join(workspace, "target", ...(target ? [target] : []), release ? "release" : "debug", name);
  const digest = createHash("sha256").update(readFileSync(source)).digest("hex");
  const manifest = { id: "faces", version: "1", hostApi: 2, targetOs, targetArch, entrypoint: name, entrypointSha256: digest };
  for (const directory of [join(workspace, "target/native/analyzers/faces"), join(workspace, "apps/desktop/src-tauri/resources/analyzers/faces")]) {
    mkdirSync(directory, { recursive: true });
    copyFileSync(source, join(directory, name));
    writeFileSync(join(directory, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  }
}

if (process.argv[1] && dirname(fileURLToPath(import.meta.url)) === dirname(process.argv[1]) && process.argv[1].endsWith("prepare-face-pack.mjs")) {
  prepareFacePack(undefined, !process.argv.includes("--debug"));
}
