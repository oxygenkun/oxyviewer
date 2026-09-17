import type { AssetKind } from "@/types";

export function requiresExiftoolSetup(kind: AssetKind, unavailable: boolean): boolean {
  return kind !== "raw" && unavailable;
}
