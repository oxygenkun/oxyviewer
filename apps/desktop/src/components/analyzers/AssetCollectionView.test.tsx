// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { AssetCollectionView } from "./AssetCollectionView";

it("renders a second collection, pages it, and executes only registered selected actions", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  const root = createRoot(container);
  const run = vi.fn(async () => {});
  const loadMore = vi.fn();
  const render = (sourceId: string) => <AssetCollectionView
    descriptor={{ id: "duplicates", source: "duplicates", selection: "multi", fields: [], actions: ["keep", "arbitrary.rpc", "toString"] }}
    sourceId={sourceId} items={["one", "two"]} itemId={(item) => item} renderItem={(item) => <span>{item}</span>}
    actions={{ keep: { label: "Keep", run } }} hasMore loadMore={loadMore} loading={false} labels={{ more: "More", select: "Select" }} />;
  try {
    await act(async () => root.render(render("duplicates")));
    expect(container.textContent).not.toContain("arbitrary.rpc");
    expect(container.textContent).not.toContain("toString");
    const button = container.querySelector("button")!;
    expect(button.disabled).toBe(true);
    await act(async () => (container.querySelector("input") as HTMLInputElement).click());
    await act(async () => button.click());
    expect(run).toHaveBeenCalledWith(["one"]);
    await act(async () => Array.from(container.querySelectorAll("button")).find((button) => button.textContent === "More")!.click());
    expect(loadMore).toHaveBeenCalledOnce();
    await act(async () => root.render(render("unregistered")));
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("Unknown collection source");
    expect(container.querySelector("button")).toBeNull();
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
