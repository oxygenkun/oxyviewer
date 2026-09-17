import { readFileSync } from "node:fs";

const rootPackage = JSON.parse(readFileSync("package.json", "utf8"));
const desktopPackage = JSON.parse(
  readFileSync("apps/desktop/package.json", "utf8"),
);
const tauriConfig = JSON.parse(
  readFileSync("apps/desktop/src-tauri/tauri.conf.json", "utf8"),
);
const cargoManifest = readFileSync("Cargo.toml", "utf8");
const workspacePackage = cargoManifest.match(
  /\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/,
)?.[1];
const cargoVersion = workspacePackage?.match(
  /^version\s*=\s*"([^"]+)"\s*$/m,
)?.[1];

const versions = new Map([
  ["package.json", rootPackage.version],
  ["apps/desktop/package.json", desktopPackage.version],
  ["Cargo.toml [workspace.package]", cargoVersion],
  ["apps/desktop/src-tauri/tauri.conf.json", tauriConfig.version],
]);
const expectedVersion = rootPackage.version;
const errors = [];

if (!/^\d+\.\d+\.\d+$/.test(expectedVersion ?? "")) {
  errors.push(`package.json has an invalid release version: ${expectedVersion}`);
}

for (const [source, version] of versions) {
  if (version !== expectedVersion) {
    errors.push(`${source} has version ${version}; expected ${expectedVersion}`);
  }
}

const releaseTag = process.argv[2];
if (releaseTag && releaseTag !== `v${expectedVersion}`) {
  errors.push(`release tag ${releaseTag} does not match v${expectedVersion}`);
}

if (errors.length > 0) {
  console.error(errors.join("\n"));
  process.exitCode = 1;
} else {
  console.log(
    `Release version ${expectedVersion} is consistent${releaseTag ? ` with tag ${releaseTag}` : ""}.`,
  );
}
