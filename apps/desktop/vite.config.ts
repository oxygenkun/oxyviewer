import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const environment = (
  globalThis as typeof globalThis & {
    process?: { env?: Record<string, string | undefined> };
  }
).process?.env;

export default defineConfig(({ command }) => {
  const host = environment?.TAURI_DEV_HOST;
  const debugBuild = command === "serve" || environment?.TAURI_ENV_DEBUG === "true";

  return {
    plugins: [react()],
    clearScreen: false,
    define: {
      __OXY_DEBUG__: JSON.stringify(debugBuild),
    },
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 1421,
          }
        : undefined,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    },
  };
});
