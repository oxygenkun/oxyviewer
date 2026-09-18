import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Downloads the pinned face-analysis models into target/native/face-models and
// stages them, with their upstream license texts, into the Tauri resource
// directory so a packaged build can bundle them.
//
// The models are content-addressed inputs, not build products: every download
// is verified against the SHA-256 recorded in source.json, and a file that is
// already present and correct is never fetched again. oxy-faces loads them by
// path; nothing in the application downloads a model at runtime, so a packaged
// build without the staged resources simply reports the feature unavailable.

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const source = JSON.parse(readFileSync(join(here, "source.json"), "utf8"));
const destination = join(root, "target", "native", "face-models");
// Bundled verbatim by apps/desktop/src-tauri/tauri.bundle.json. The application
// resolves this directory through `app.path().resource_dir()`.
const resources = join(root, "apps", "desktop", "src-tauri", "resources", "face-models");

function digest(data) {
  return createHash("sha256").update(data).digest("hex");
}

function verified(path, expected) {
  return existsSync(path) && digest(readFileSync(path)) === expected;
}

async function download(url) {
  const response = await fetch(url, {
    headers: { accept: "application/octet-stream" },
    redirect: "follow",
  });
  if (!response.ok) {
    throw new Error(`${url} responded ${response.status} ${response.statusText}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

async function fetchVerified({ url, sha256, sizeBytes, file }) {
  const bytes = await download(url);
  const actual = digest(bytes);
  if (actual !== sha256) {
    throw new Error(`${file} checksum mismatch: expected ${sha256}, received ${actual}`);
  }
  if (bytes.byteLength !== sizeBytes) {
    throw new Error(
      `${file} size mismatch: expected ${sizeBytes}, received ${bytes.byteLength}`,
    );
  }
  return bytes;
}

export async function prepare() {
  mkdirSync(destination, { recursive: true });
  mkdirSync(resources, { recursive: true });
  for (const model of source.models) {
    const target = join(destination, model.file);
    if (verified(target, model.sha256)) {
      console.log(`face-models: ${model.file} already verified`);
    } else {
      process.stdout.write(`face-models: downloading ${model.file} (${model.license})\n`);
      writeFileSync(
        target,
        await fetchVerified({
          url: model.url,
          sha256: model.sha256,
          sizeBytes: model.sizeBytes,
          file: model.file,
        }),
      );
      console.log(`face-models: wrote ${target}`);
    }
    // The staged copy is what a release package ships.
    copyFileSync(target, join(resources, model.file));

    const license = model.licenseFile;
    const licenseTarget = join(destination, license.file);
    if (!verified(licenseTarget, license.sha256)) {
      process.stdout.write(`face-models: downloading ${license.file} (${model.license})\n`);
      writeFileSync(
        licenseTarget,
        await fetchVerified({
          url: license.url,
          sha256: license.sha256,
          sizeBytes: license.sizeBytes,
          file: license.file,
        }),
      );
    }
    copyFileSync(licenseTarget, join(resources, license.file));
  }
  // A provenance note travels with the binaries, so a packaged application can
  // state which files it ships, from where, and under which license.
  writeFileSync(
    join(resources, "NOTICE.md"),
    [
      "# Bundled face model files",
      "",
      "These files are downloaded inputs staged by `3rdpart/face-models/prepare.mjs`",
      "and pinned by the SHA-256 in `source.json`. Upstream license texts are beside",
      "them. See `THIRD_PARTY_NOTICES.md` for the full component list.",
      "",
      "| File | Version | License | SHA-256 |",
      "| --- | --- | --- | --- |",
      ...source.models.flatMap((model) => [
        `| \`${model.file}\` | ${model.version} | ${model.license} | \`${model.sha256}\` |`,
        `| \`${model.licenseFile.file}\` | ${model.version} | ${model.license} | \`${model.licenseFile.sha256}\` |`,
      ]),
      "",
      `Detector input size: ${source.detectorInputSize}`,
      "",
    ].join("\n"),
  );

  const manifest = join(destination, "face-models.json");
  writeFileSync(
    manifest,
    `${JSON.stringify(
      {
        detectorInputSize: source.detectorInputSize,
        models: source.models.map((model) => ({
          id: model.id,
          version: model.version,
          file: model.file,
          sha256: model.sha256,
          license: model.license,
        })),
      },
      null,
      2,
    )}\n`,
  );
  console.log(`face-models: ready under ${destination}`);
}

const invokedDirectly =
  process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url));

if (invokedDirectly) {
  prepare().catch((error) => {
    console.error(`face-models: ${error.message}`);
    process.exitCode = 1;
  });
}
