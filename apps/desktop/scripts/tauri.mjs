import { chmodSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { prepare } from "../../../3rdpart/ffmpeg/prepare.mjs";
import { prepare as prepareLibheif } from "../../../3rdpart/libheif/prepare.mjs";

const require = createRequire(import.meta.url);
const cli = require("@tauri-apps/cli");

// Tauri CLI 2.11.x can leave this helper empty and non-executable on macOS,
// which prevents beforeDevCommand descendants such as Vite from being reaped.
// Keep this byte-for-byte aligned with the CLI's bundled kill-children.sh.
// Remove this workaround after https://github.com/tauri-apps/tauri/issues/15098
// is fixed in the minimum supported CLI version.
const killChildrenScript = `#!/usr/bin/env sh

getcpid() {
    cpids=$(pgrep -P $1|xargs)
    for cpid in $cpids;
    do
        echo "$cpid"
        getcpid $cpid
    done
}

kill $(getcpid $1)
`;

if (process.platform !== "win32") {
  const helperPath = join(tmpdir(), "tauri-stop-dev-processes.sh");
  writeFileSync(helperPath, killChildrenScript, { mode: 0o770 });
  chmodSync(helperPath, 0o770);
}

try {
  const args = process.argv.slice(2);
  if (["build", "bundle"].includes(args[0]) && !args.includes("--help") && !args.includes("-h")) {
    const targetIndex = args.findIndex((arg) => arg === "--target" || arg === "-t");
    const target = targetIndex >= 0 ? args[targetIndex + 1] : args.find((arg) => arg.startsWith("--target="))?.slice(9);
    prepare(target);
    prepareLibheif(target);
    const separator = args.indexOf("--");
    args.splice(separator < 0 ? args.length : separator, 0, "--config", fileURLToPath(new URL("../src-tauri/tauri.bundle.json", import.meta.url)));
  }
  await cli.run(args, "pnpm tauri");
} catch (error) {
  cli.logError(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
