import { describe, expect, it } from "vitest";
import {
  isSameOrDescendantPath,
  parentFolderPath,
  platformFileManager,
  relativeFolderPath,
} from "./folderPaths";

describe("folder paths", () => {
  it("copies paths relative to a POSIX library root", () => {
    expect(relativeFolderPath("/photos", "/photos")).toBe(".");
    expect(relativeFolderPath("/photos", "/photos/trips/coast")).toBe("trips/coast");
  });

  it("preserves Windows separators and compares drive paths case-insensitively", () => {
    expect(relativeFolderPath("C:\\Photos", "c:\\Photos\\Trips\\Coast")).toBe("Trips\\Coast");
    expect(parentFolderPath("C:\\Photos\\Trips")).toBe("C:\\Photos");
  });

  it("does not confuse similarly prefixed sibling paths for descendants", () => {
    expect(relativeFolderPath("/photos", "/photos-old")).toBe("/photos-old");
    expect(isSameOrDescendantPath("/photos/trips", "/photos/trips/coast")).toBe(true);
    expect(isSameOrDescendantPath("/photos/trips", "/photos/triptych")).toBe(false);
  });

  it("uses the platform-specific file manager name", () => {
    expect(platformFileManager("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("finder");
    expect(platformFileManager("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windowsExplorer");
    expect(platformFileManager("Mozilla/5.0 (X11; Linux x86_64)")).toBe("generic");
  });
});
