import { describe, expect, it } from "vitest";
import { requiresExiftoolSetup } from "./metadataProvider";

describe("embedded metadata synchronization", () => {
  it("prompts only for unavailable explicit embedded sync", () => {
    expect(requiresExiftoolSetup("heif", true)).toBe(true);
    expect(requiresExiftoolSetup("jpeg", true)).toBe(true);
    expect(requiresExiftoolSetup("raw", true)).toBe(false);
    expect(requiresExiftoolSetup("heif", false)).toBe(false);
  });
});
