import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { FolderSession } from "../types";
import { insertRestoredFolder, restoreFoldersProgressively, type FolderRestoreState } from "./folderRestoration";

const folder = (rootPath: string): FolderSession => ({
  id: rootPath, rootPath, displayName: rootPath, openedAtMs: 0,
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe("progressive folder restoration", () => {
  it("publishes the active root while a slow root is still pending", async () => {
    const slow = deferred<FolderSession>();
    const active = deferred<FolderSession>();
    const open = vi.fn((path: string) => path === "active" ? active.promise : slow.promise);
    const published = vi.fn();
    let states: FolderRestoreState[] = [];
    const restore = restoreFoldersProgressively(["slow", "active"], "active", open, published,
      (next) => { states = next; });
    expect(open.mock.calls.map(([path]) => path)).toEqual(["active", "slow"]);
    active.resolve(folder("active"));
    await active.promise;
    expect(published).toHaveBeenCalledWith(folder("active"));
    expect(states.map((state) => state.status)).toEqual(["restoring", "ready"]);
    slow.reject(new Error("NAS offline"));
    await restore;
    expect(states[0]).toMatchObject({ status: "failed", error: "Error: NAS offline" });
    expect(published).toHaveBeenCalledTimes(1);
  });

  it("keeps other roots available even while the saved active root is offline", async () => {
    const active = deferred<FolderSession>();
    const published = vi.fn();
    const restore = restoreFoldersProgressively(["active", "local"], "active",
      (path) => path === "active" ? active.promise : Promise.resolve(folder(path)), published, () => {});
    await Promise.resolve();
    expect(published).toHaveBeenCalledWith(folder("local"));
    active.reject("offline");
    await restore;
  });

  it("preserves import order and folders added during restoration", () => {
    const roots = ["first", "second", "active"];
    let sessions = [folder("active"), folder("new")];
    sessions = insertRestoredFolder(sessions, folder("second"), roots);
    sessions = insertRestoredFolder(sessions, folder("first"), roots);
    expect(sessions.map((session) => session.rootPath)).toEqual([...roots, "new"]);
    expect(insertRestoredFolder(sessions, folder("active"), roots)).toBe(sessions);
  });

  it("does not resurrect a removed ready folder when the query completes", async () => {
    const client = new QueryClient();
    const slow = deferred<FolderSession>();
    const key = ["open-folders"];
    const query = client.fetchQuery({ queryKey: key, queryFn: async () => {
      await restoreFoldersProgressively(["active", "slow"], "active",
        (path) => path === "slow" ? slow.promise : Promise.resolve(folder(path)),
        (session) => client.setQueryData<FolderSession[]>(key, (current = []) =>
          insertRestoredFolder(current, session, ["active", "slow"])), () => {});
      return client.getQueryData<FolderSession[]>(key) ?? [];
    } });
    await Promise.resolve();
    expect(client.getQueryData(key)).toEqual([folder("active")]);
    expect(client.getQueryState(key)?.fetchStatus).toBe("fetching");
    client.setQueryData(key, []);
    slow.resolve(folder("slow"));
    await query;
    expect(client.getQueryData(key)).toEqual([folder("slow")]);
    client.clear();
  });

  it("handles an empty library without opening a folder", async () => {
    const open = vi.fn();
    const report = vi.fn();
    await restoreFoldersProgressively([], undefined, open, vi.fn(), report);
    expect(open).not.toHaveBeenCalled();
    expect(report).toHaveBeenCalledWith([]);
  });
});
