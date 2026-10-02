import { expect, it, vi } from "vitest";
import type { FolderSession } from "@/types";
import { resolvePersonFolder } from "./openPersonFolder";

const session = (rootPath: string) => ({ id: rootPath, rootPath }) as FolderSession;

it("reuses the closest existing root, including a nested UNC directory", async () => {
  const root = "\\\\?\\UNC\\nas\\photo";
  const check = vi.fn().mockResolvedValue(undefined);
  const register = vi.fn();
  const nested = session(`${root}\\2026`);
  const path = `${root}\\2026\\演出\\04 ReNus`;
  expect(await resolvePersonFolder(path, [session(root), nested], check, register))
    .toEqual({ session: nested, path });
  expect(check).toHaveBeenCalledWith(path);
  expect(register).not.toHaveBeenCalled();
});

it("registers an unopened directory and uses the returned canonical path", async () => {
  const opened = session("C:\\photos-old");
  const register = vi.fn().mockResolvedValue(opened);
  expect(await resolvePersonFolder("C:/photos-old", [session("C:/photos")], vi.fn(), register))
    .toEqual({ session: opened, path: opened.rootPath });
  expect(register).toHaveBeenCalledOnce();
});

it("does not register or navigate when the source is offline", async () => {
  const register = vi.fn();
  await expect(resolvePersonFolder("C:/offline", [], vi.fn().mockRejectedValue(new Error("offline")), register))
    .rejects.toThrow("offline");
  expect(register).not.toHaveBeenCalled();
});
