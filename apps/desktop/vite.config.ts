import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import desktopPackage from "./package.json";

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
      __OXY_APP_VERSION__: JSON.stringify(desktopPackage.version),
    },
    server: {
      port: 15142,
      strictPort: true,
      // On some Windows configurations, localhost resolves to IPv6 (::1),
      // which can fail with EACCES even when the IPv4 loopback is available.
      host: host || "127.0.0.1",
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 15143,
          }
        : undefined,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    },
  };
});
