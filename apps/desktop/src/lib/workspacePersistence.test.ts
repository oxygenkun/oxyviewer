import { describe, expect, it } from "vitest";
import { parseWorkspaceSnapshot } from "./workspacePersistence";

describe("workspace persistence", () => {
  it("restores the active root and each root's last directory", () => {
    expect(parseWorkspaceSnapshot(JSON.stringify({
      activeRoot: "/photos",
      currentDirectories: { "/photos": "/photos/2025", "/archive": "/archive" },
    }))).toEqual({
      activeRoot: "/photos",
      currentDirectories: { "/photos": "/photos/2025", "/archive": "/archive" },
    });
  });

  it("fails closed for malformed or obsolete data", () => {
    expect(parseWorkspaceSnapshot("not json")).toEqual({ currentDirectories: {} });
    expect(parseWorkspaceSnapshot('{"currentDirectories":[]}')).toEqual({ currentDirectories: {} });
  });
});
