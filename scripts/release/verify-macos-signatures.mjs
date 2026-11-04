import { execFileSync } from "node:child_process";
import { mkdtempSync, rmdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

function verifyApp(appPath) {
  // Check sidecars explicitly as well as the resource seal on the whole app.
  for (const executable of ["oxyviewer", "oxy-ffmpeg", "oxy-ffprobe"]) {
    execFileSync("codesign", [
      "--verify", "--strict", "--verbose=2",
      join(appPath, "Contents", "MacOS", executable),
    ], { stdio: "inherit" });
  }
  execFileSync("codesign", [
    "--verify", "--deep", "--strict", "--verbose=2", appPath,
  ], { stdio: "inherit" });
  console.log(`Verified macOS signatures: ${appPath}`);
}

function verifyDmg(dmgPath) {
  const mountPath = mkdtempSync(join(tmpdir(), "oxyviewer-signatures-"));
  let mounted = false;
  try {
    // The DMG embeds the repository license. Accept its mount prompt without
    // installing or running the application; keep the image read-only.
    execFileSync("hdiutil", [
      "attach", "-readonly", "-nobrowse", "-mountpoint", mountPath, dmgPath,
    ], { input: "Y\n", stdio: ["pipe", "inherit", "inherit"] });
    mounted = true;
    verifyApp(join(mountPath, "OxyViewer.app"));
  } finally {
    if (mounted) {
      execFileSync("hdiutil", ["detach", mountPath], { stdio: "inherit" });
    }
    rmdirSync(mountPath);
  }
}

try {
  const paths = process.argv.slice(2);
  if (process.platform !== "darwin" || paths.length === 0) {
    throw new Error("Usage (macOS): node scripts/release/verify-macos-signatures.mjs <app-or-dmg> [...]");
  }
  for (const path of paths) {
    const absolutePath = resolve(path);
    if (absolutePath.endsWith(".app")) {
      verifyApp(absolutePath);
    } else if (absolutePath.endsWith(".dmg")) {
      verifyDmg(absolutePath);
    } else {
      throw new Error(`Expected a .app or .dmg: ${path}`);
    }
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
