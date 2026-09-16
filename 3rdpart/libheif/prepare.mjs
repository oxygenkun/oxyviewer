import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, copyFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { prepare as prepareFfmpeg } from "../ffmpeg/prepare.mjs";

// Builds the pinned libheif for macOS/Linux as a static library with the FFmpeg
// decoder, linked against the pinned FFmpeg static prefix. Windows links
// libheif through the pinned vcpkg revision (libde265 decoder) instead.

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const source = JSON.parse(readFileSync(join(here, "source.json"), "utf8"));
const sha = (data) => createHash("sha256").update(data).digest("hex");
const nativePrefix = join(root, "target", "native");
const ffmpegPrefix = join(nativePrefix, "ffmpeg");
const installPrefix = join(nativePrefix, "libheif");

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", ...options });
  if (result.error || result.status !== 0) {
    throw new Error(`${program} failed: ${result.error?.message ?? result.stderr ?? result.status}`);
  }
  return `${result.stdout ?? ""}${result.stderr ?? ""}`;
}

// Upstream 1.23.4 never adds its FFmpeg dependency to libheif.pc: the
// CMakeLists.txt check reads FFMPEG_DECODER_FOUND, which no code path sets, so
// `Requires.private` stays empty while the static archive still needs libavcodec
// and libavutil. Expand the pinned FFmpeg flags into the installed .pc so
// libheif-sys links the archive and its dependencies through pkg-config alone.
function patchPkgConfig(pkgConfigPath) {
  const text = readFileSync(pkgConfigPath, "utf8");
  const privateLibs = /^Libs\.private:(.*)$/m.exec(text)?.[1].trim() ?? "";
  const ffmpegFlags = run("pkg-config", ["--libs", "libavcodec", "libavutil"], {
    env: { ...process.env, PKG_CONFIG_PATH: join(ffmpegPrefix, "lib", "pkgconfig") },
  }).trim();
  if (!ffmpegFlags.includes("-lavcodec")) {
    throw new Error("pkg-config did not report libavcodec; the FFmpeg prefix is incomplete");
  }
  const patched = text
    .replace(
      /^Libs\.private:.*$/m,
      "# Patched by 3rdpart/libheif/prepare.mjs: upstream omits the FFmpeg dependency\n" +
        "Libs.private: " +
        privateLibs,
    )
    .replace(
      /^Libs:.*$/m,
      `Libs: -L\${libdir} -lheif ${[privateLibs, ffmpegFlags].filter(Boolean).join(" ")}`,
    );
  writeFileSync(pkgConfigPath, patched);
}

function verifyPkgConfig() {
  const libs = run("pkg-config", ["--libs", "libheif"], {
    env: { ...process.env, PKG_CONFIG_PATH: join(installPrefix, "lib", "pkgconfig") },
  });
  for (const required of ["-lheif", "-lavcodec", "-lavutil"]) {
    if (!libs.includes(required)) {
      throw new Error(`libheif.pc is missing ${required}; run pnpm heif:prepare again`);
    }
  }
  for (const relative of ["include/libheif/heif.h", "lib/libheif.a"]) {
    if (!existsSync(join(installPrefix, relative))) {
      throw new Error(`libheif prefix is missing ${relative}`);
    }
  }
}

// Stage the upstream license text and build recipe beside the FFmpeg material
// so every package that links libheif carries its LGPL terms and corresponds to
// rebuildable source. Windows stages the same files without building anything.
function stageLicenseMaterials() {
  const resources = join(root, "apps", "desktop", "src-tauri", "resources", "libheif");
  mkdirSync(resources, { recursive: true });
  for (const name of ["COPYING", "README.md", "source.json", "build.sh", "prepare.mjs"]) {
    copyFileSync(join(here, name), join(resources, name));
  }
}

export function prepare(requestedTarget) {
  stageLicenseMaterials();
  if (process.platform === "win32") {
    // Keep the cache path present even though Windows installs nothing here.
    mkdirSync(nativePrefix, { recursive: true });
    console.log("libheif on Windows comes from the pinned vcpkg revision; nothing to prepare.");
    return;
  }
  const host = run("rustc", ["--print", "host-tuple"]).trim();
  const target = requestedTarget ?? host;
  if (target !== host) {
    throw new Error(`libheif requires a native build for ${target}; host is ${host}. Cross/universal builds are not configured.`);
  }
  const ffmpegReceiptPath = join(root, "target", "ffmpeg", target, "receipt.json");
  if (!existsSync(ffmpegReceiptPath) || !existsSync(join(ffmpegPrefix, "lib", "pkgconfig", "libavcodec.pc"))) {
    console.log("Preparing the pinned FFmpeg static prefix first");
    prepareFfmpeg(requestedTarget);
  }
  const ffmpegRecipe = JSON.parse(readFileSync(ffmpegReceiptPath, "utf8")).recipe;

  const cache = join(root, "target", "libheif", target);
  const archive = join(cache, `libheif-${source.version}.tar.gz`);
  const sourceDir = join(cache, `libheif-${source.version}`);
  const buildDir = join(cache, "build");
  const receipt = join(cache, "receipt.json");
  const recipe = sha(
    Buffer.concat([
      readFileSync(join(here, "source.json")),
      readFileSync(join(here, "build.sh")),
      readFileSync(fileURLToPath(import.meta.url)),
      Buffer.from(`${target}\n${ffmpegRecipe}`),
    ]),
  );
  mkdirSync(cache, { recursive: true });
  if (!existsSync(archive)) {
    const partial = `${archive}.download`;
    run("curl", ["--fail", "--location", "--retry", "3", "--output", partial, source.url], { stdio: "inherit" });
    if (sha(readFileSync(partial)) !== source.sha256) throw new Error("libheif source checksum mismatch");
    copyFileSync(partial, archive);
    rmSync(partial);
  }
  if (sha(readFileSync(archive)) !== source.sha256) throw new Error("Cached libheif source checksum mismatch");

  let cached;
  try {
    cached = JSON.parse(readFileSync(receipt, "utf8"));
  } catch {
    /* First build. */
  }
  const reusable = cached?.recipe === recipe
    && existsSync(join(sourceDir, "CMakeLists.txt"))
    && existsSync(join(installPrefix, "lib", "libheif.a"));
  if (!reusable) {
    rmSync(buildDir, { recursive: true, force: true });
    rmSync(sourceDir, { recursive: true, force: true });
    rmSync(installPrefix, { recursive: true, force: true });
    run("tar", ["-xf", archive, "-C", cache]);
    // The distributed license text must match the source we actually link.
    if (sha(readFileSync(join(here, "COPYING"))) !== sha(readFileSync(join(sourceDir, "COPYING")))) {
      throw new Error("3rdpart/libheif/COPYING does not match the pinned libheif source");
    }
    // heifio only serves the example programs, but libheif adds it
    // unconditionally and it pulls optional host image libraries (PNG, WebP,
    // TIFF) into the build. Drop it so the static prefix links nothing beyond
    // the pinned FFmpeg libraries and the C++ runtime.
    const cmakeLists = join(sourceDir, "CMakeLists.txt");
    writeFileSync(cmakeLists, readFileSync(cmakeLists, "utf8").replace(/^add_subdirectory\(heifio\)\n/m, ""));
    console.log(`Building libheif ${source.version} for ${target} with the FFmpeg decoder`);
    run("bash", [join(here, "build.sh"), target, sourceDir, buildDir, ffmpegPrefix, installPrefix], {
      stdio: "inherit",
    });
    const cacheFile = readFileSync(join(buildDir, "CMakeCache.txt"), "utf8");
    if (!cacheFile.includes(`FFMPEG_avcodec_LIBRARY:FILEPATH=${ffmpegPrefix}`)) {
      throw new Error("libheif did not link the pinned FFmpeg prefix; remove a system FFmpeg from the search path");
    }
    patchPkgConfig(join(installPrefix, "lib", "pkgconfig", "libheif.pc"));
  }
  verifyPkgConfig();
  writeFileSync(receipt, `${JSON.stringify({ ...source, target, recipe }, null, 2)}\n`);
  console.log(`Verified pinned libheif ${source.version} static prefix: ${installPrefix}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    prepare(process.argv[2]);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
