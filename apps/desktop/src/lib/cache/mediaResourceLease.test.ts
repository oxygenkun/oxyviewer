import { beforeEach, describe, expect, it, vi } from "vitest";
import { releaseMediaResource } from "@/lib/api";
import { releaseUnretainedMediaResource, retainMediaResource } from "./mediaResourceLease";

vi.mock("@/lib/api", () => ({ releaseMediaResource: vi.fn(async () => {}) }));

beforeEach(() => {
  vi.mocked(releaseMediaResource).mockClear();
});

describe("shared media resource ownership", () => {
  it("releases only after both thumbnail and loupe relinquish their leases", async () => {
    const thumbnail = retainMediaResource("shared");
    const loupe = retainMediaResource("shared");
    thumbnail();
    thumbnail(); // Cleanup is idempotent.
    releaseUnretainedMediaResource("shared"); // A late result is not an owner.
    await Promise.resolve();
    expect(releaseMediaResource).not.toHaveBeenCalled();
    loupe();
    await Promise.resolve();
    expect(releaseMediaResource).toHaveBeenCalledExactlyOnceWith("shared");
  });

  it("does not release between StrictMode cleanup and reacquisition", async () => {
    retainMediaResource("strict")();
    const next = retainMediaResource("strict");
    await Promise.resolve();
    expect(releaseMediaResource).not.toHaveBeenCalled();
    next();
    await Promise.resolve();
    expect(releaseMediaResource).toHaveBeenCalledExactlyOnceWith("strict");
  });

  it("coalesces late unclaimed results and tolerates failed teardown IPC", async () => {
    vi.mocked(releaseMediaResource).mockRejectedValueOnce(new Error("app shutdown"));
    releaseUnretainedMediaResource("late");
    releaseUnretainedMediaResource("late");
    await Promise.resolve();
    expect(releaseMediaResource).toHaveBeenCalledExactlyOnceWith("late");
  });
});
