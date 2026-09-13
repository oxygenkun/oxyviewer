import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const source = JSON.parse(readFileSync(join(here, "source.json"), "utf8"));
const sha = (data) => createHash("sha256").update(data).digest("hex");

function run(program, args, options = {}) {
  const result = spawnSync(program, args, { encoding: "utf8", ...options });
  if (result.error || result.status !== 0) {
    throw new Error(`${program} failed: ${result.error?.message ?? result.stderr ?? result.status}`);
  }
  return `${result.stdout ?? ""}${result.stderr ?? ""}`;
}

export function verifyPair(ffmpeg, ffprobe) {
  const execute = (program, args) => run(program, args, { env: { ...process.env, PATH: "" } });
  for (const executable of [ffmpeg, ffprobe]) {
    const version = execute(executable, ["-version"]);
    if (!version.includes(`version ${source.version}`)) throw new Error(`Unexpected version: ${executable}`);
    if (/--enable-(gpl|nonfree|version3)\b/.test(version)) throw new Error(`Unexpected license configuration: ${executable}`);
  }
  const checks = [
    [ffmpeg, "-decoders", ["hevc", "mjpeg", "rawvideo"]],
    [ffmpeg, "-encoders", ["mjpeg", "bmp"]],
    [ffmpeg, "-demuxers", ["mov", "rawvideo"]],
    [ffmpeg, "-muxers", ["image2", "image2pipe"]],
    [ffmpeg, "-filters", ["scale", "format", "crop", "transpose", "hflip", "vflip", "unsharp", "xstack"]],
    [ffprobe, "-h", ["-show_stream_groups"]],
  ];
  for (const [executable, option, required] of checks) {
    const output = execute(executable, [option]);
    for (const name of required) {
      if (!output.includes(name)) throw new Error(`${executable} lacks ${name}`);
    }
  }
  // Exercise the actual filter/encoding paths without a system FFmpeg or a
  // private camera fixture. Real HEIF decode is covered by OXY_HIF_FIXTURE.
  const temporary = mkdtempSync(join(tmpdir(), "oxy-ffmpeg-smoke-"));
  try {
    const input = join(temporary, "input.rgb");
    writeFileSync(input, Buffer.alloc(32 * 32 * 3, 100));
    for (const [codec, extension, pixelFormat] of [["mjpeg", "jpg", "yuvj444p"], ["bmp", "bmp", "bgra"]]) {
      const output = join(temporary, `output.${extension}`);
      execute(ffmpeg, ["-v", "error", "-f", "rawvideo", "-pixel_format", "rgb24", "-video_size", "32x32", "-i", input,
        "-filter_complex", `[0:v]split[a][b];[a][b]xstack=inputs=2:layout=0_0|32_0,crop=60:30,transpose=clock,hflip,vflip,unsharp=3:3:0.2:3:3:0,scale=32:64,format=${pixelFormat}[out]`,
        "-map", "[out]", "-frames:v", "1", "-c:v", codec, "-threads", "1", "-y", output]);
      const bytes = readFileSync(output);
      if (codec === "mjpeg" ? bytes[0] !== 0xff || bytes[1] !== 0xd8 : bytes.toString("ascii", 0, 2) !== "BM") {
        throw new Error(`Invalid ${codec} smoke output`);
      }
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

function verifyBundle(directory) {
  const files = [];
  function visit(folder) {
    for (const entry of readdirSync(folder, { withFileTypes: true })) {
      const path = join(folder, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile()) files.push(path);
    }
  }
  visit(directory);
  const extension = process.platform === "win32" ? ".exe" : "";
  const programs = files.filter((path) => path.endsWith(`/oxy-ffmpeg${extension}`) || path.endsWith(`\\oxy-ffmpeg${extension}`));
  if (programs.length !== 1) throw new Error(`Expected exactly one bundled FFmpeg in ${directory}, found ${programs.length}`);
  verifyPair(programs[0], join(dirname(programs[0]), `oxy-ffprobe${extension}`));
  const archives = files.filter((path) => path.endsWith(`ffmpeg-${source.version}.tar.xz`));
  if (archives.length !== 0) throw new Error("FFmpeg source archive must be published separately, not bundled in the application");
  const manifests = files.filter((path) => path.endsWith("/licenses/ffmpeg/source.json") || path.endsWith("\\licenses\\ffmpeg\\source.json"));
  if (manifests.length !== 1) throw new Error(`Expected one installed FFmpeg source manifest, found ${manifests.length}`);
  const installedSource = JSON.parse(readFileSync(manifests[0], "utf8"));
  if (installedSource.version !== source.version || installedSource.sha256 !== source.sha256 || installedSource.url !== source.url) {
    throw new Error("Installed FFmpeg source manifest does not match the bundled programs");
  }
  for (const name of ["COPYING.LGPLv2.1", "LICENSE.md", "README.md", "build.sh", "source.json", "prepare.mjs", "config.h", "configure-summary.txt", "build-info.json"]) {
    if (!existsSync(join(dirname(manifests[0]), name))) throw new Error(`Missing bundled FFmpeg notice/build material: ${name}`);
  }
  console.log(`Verified installed FFmpeg and license/build materials in ${directory}`);
}

function verifySourceArchive(path) {
  if (!existsSync(path) || sha(readFileSync(path)) !== source.sha256) {
    throw new Error(`Corresponding FFmpeg source archive missing or corrupt: ${path}`);
  }
  console.log(`Verified corresponding FFmpeg source archive: ${path}`);
}

export function prepare(requestedTarget) {
  const host = run("rustc", ["--print", "host-tuple"]).trim();
  const target = requestedTarget ?? host;
  const supported = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];
  if (!supported.includes(target) || target !== host) {
    throw new Error(`FFmpeg requires a native build for ${target}; host is ${host}. Cross/universal builds are not configured.`);
  }
  const extension = target.includes("windows") ? ".exe" : "";
  const cache = join(root, "target", "ffmpeg", target);
  const binaries = join(root, "apps/desktop/src-tauri/binaries");
  const resources = join(root, "apps/desktop/src-tauri/resources/ffmpeg");
  const archiveName = `ffmpeg-${source.version}.tar.xz`;
  const archive = join(cache, archiveName);
  const sourceDir = join(cache, `ffmpeg-${source.version}`);
  const buildDir = join(cache, "build");
  const ffmpeg = join(binaries, `oxy-ffmpeg-${target}${extension}`);
  const ffprobe = join(binaries, `oxy-ffprobe-${target}${extension}`);
  const receipt = join(cache, "receipt.json");
  const recipe = sha(Buffer.concat([readFileSync(join(here, "source.json")), readFileSync(join(here, "build.sh")), readFileSync(fileURLToPath(import.meta.url)), Buffer.from(target)]));
  mkdirSync(cache, { recursive: true });
  if (!existsSync(archive)) {
    const partial = `${archive}.download`;
    run("curl", ["--fail", "--location", "--retry", "3", "--output", partial, source.url], { stdio: "inherit" });
    if (sha(readFileSync(partial)) !== source.sha256) throw new Error("FFmpeg source checksum mismatch");
    copyFileSync(partial, archive);
    rmSync(partial);
  }
  if (sha(readFileSync(archive)) !== source.sha256) throw new Error("Cached FFmpeg source checksum mismatch");
  let cached;
  try { cached = JSON.parse(readFileSync(receipt, "utf8")); } catch { /* First build. */ }
  const reusable = cached?.recipe === recipe
    && existsSync(join(sourceDir, "COPYING.LGPLv2.1")) && existsSync(join(buildDir, "config.h"))
    && [ffmpeg, ffprobe].every((path) => existsSync(path) && sha(readFileSync(path)) === cached.binaries?.[path]);
  if (!reusable) {
    rmSync(buildDir, { recursive: true, force: true });
    rmSync(sourceDir, { recursive: true, force: true });
    run("tar", ["-xf", archive, "-C", cache]);
    const bash = process.platform === "win32" ? process.env.OXY_FFMPEG_BASH ?? "C:/msys64/usr/bin/bash.exe" : "bash";
    console.log(`Building FFmpeg ${source.version} for ${target} (first build may take several minutes)`);
    const shellPath = (path) => process.platform === "win32" ? path.replaceAll("\\", "/") : path;
    run(bash, [...(process.platform === "win32" ? ["--login"] : []), shellPath(join(here, "build.sh")), target, shellPath(sourceDir), shellPath(buildDir)], {
      stdio: "inherit", env: { ...process.env, MSYSTEM: "UCRT64", CHERE_INVOKING: "1" },
    });
    mkdirSync(binaries, { recursive: true });
    for (const [name, destination] of [["ffmpeg", ffmpeg], ["ffprobe", ffprobe]]) {
      copyFileSync(join(buildDir, `${name}${extension}`), destination);
      chmodSync(destination, 0o755);
    }
  }
  verifyPair(ffmpeg, ffprobe);
  mkdirSync(resources, { recursive: true });
  for (const name of ["COPYING.LGPLv2.1", "LICENSE.md"]) copyFileSync(join(sourceDir, name), join(resources, name));
  for (const name of ["README.md", "build.sh", "source.json", "prepare.mjs"]) copyFileSync(join(here, name), join(resources, name));
  copyFileSync(join(buildDir, "config.h"), join(resources, "config.h"));
  copyFileSync(join(buildDir, "configure-summary.txt"), join(resources, "configure-summary.txt"));
  const result = { ...source, target, recipe, binaries: Object.fromEntries([ffmpeg, ffprobe].map((path) => [path, sha(readFileSync(path))])) };
  writeFileSync(receipt, `${JSON.stringify(result, null, 2)}\n`);
  writeFileSync(join(resources, "build-info.json"), `${JSON.stringify({ ...source, target, recipe, configuration: run(ffmpeg, ["-version"]) }, null, 2)}\n`);
  console.log(`Verified bundled FFmpeg ${source.version}: ${target}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv[2] === "--verify-bundle") {
      verifyBundle(resolve(process.argv[3]));
    } else if (process.argv[2] === "--verify-source") {
      verifySourceArchive(resolve(process.argv[3]));
    } else if (process.argv[2] === "--verify-dir") {
      const directory = resolve(process.argv[3]);
      const extension = process.platform === "win32" ? ".exe" : "";
      verifyPair(join(directory, `oxy-ffmpeg${extension}`), join(directory, `oxy-ffprobe${extension}`));
      console.log(`Verified installed FFmpeg pair in ${directory}`);
    } else {
      prepare(process.argv[2]);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
