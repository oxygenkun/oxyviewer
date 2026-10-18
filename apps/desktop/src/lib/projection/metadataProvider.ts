import type { AssetKind } from "@/types";

export type ExiftoolSetupAction = "download" | "select" | "check";

export function requiresExiftoolSetup(kind: AssetKind, unavailable: boolean): boolean {
  return kind !== "raw" && unavailable;
}
