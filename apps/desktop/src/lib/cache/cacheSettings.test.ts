import { describe, expect, it } from "vitest";
import { GIB, cacheLimitGb, formatBytes, validCacheLimitGb } from "./cacheSettings";

describe("cache settings", () => {
  it("formats storage usage at a useful precision", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1536)).toBe("1.50 KB");
    expect(formatBytes(12.5 * GIB)).toBe("12.5 GB");
  });

  it("converts and validates the supported capacity range", () => {
    expect(cacheLimitGb(10 * GIB)).toBe(10);
    expect(validCacheLimitGb(1)).toBe(true);
    expect(validCacheLimitGb(500)).toBe(true);
    expect(validCacheLimitGb(0)).toBe(false);
    expect(validCacheLimitGb(1.5)).toBe(false);
  });
});
