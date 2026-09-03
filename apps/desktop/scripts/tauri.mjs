import { chmodSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";

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
  await cli.run(process.argv.slice(2), "pnpm tauri");
} catch (error) {
  cli.logError(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
