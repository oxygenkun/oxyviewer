// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { App } from "./App";
import { chooseFolder, openFolder } from "@/lib/api";
import { loadFolderOnboarding } from "@/lib/browse/workspacePersistence";
import { useWorkspaceStore } from "@/store";

vi.mock("@/lib/api", async (importOriginal) => {
  const api = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...api,
    addLibraryRoot: vi.fn(async () => ["/demo/Field Notes"]),
    chooseFolder: vi.fn(),
    openFolder: vi.fn(api.openFolder),
    preloadAssetThumbnail: vi.fn(async () => undefined),
  };
});

it("anchors each getting-started step and replays the sequence with or without an open folder", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const userAgent = vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue("Mozilla/5.0 (Macintosh)");
  vi.stubGlobal("ResizeObserver", class {
    observe() {}
    unobserve() {}
    disconnect() {}
  });
  localStorage.clear();
  const initialState = useWorkspaceStore.getState();
  useWorkspaceStore.setState({ locale: "en", leftPanelOpen: false, settingsOpen: false });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const settle = () => act(async () => { await vi.advanceTimersByTimeAsync(50); });
  const render = async (key: string) => {
    await act(async () => root.render(<QueryClientProvider client={client}><App key={key} /></QueryClientProvider>));
    await settle();
  };
  const guide = () => document.querySelector<HTMLElement>("[data-folder-guide]");
  const click = async (label: string) => {
    const button = [...document.querySelectorAll("button")].find((node) =>
      node.textContent?.trim() === label || node.dataset.guideAnchor === label);
    expect(button).toBeDefined();
    await act(async () => button!.click());
    await settle();
  };
  const resetGuide = async () => {
    await act(async () => useWorkspaceStore.getState().openSettings());
    await click("Reset tips");
    expect(useWorkspaceStore.getState().settingsOpen).toBe(false);
    expect(loadFolderOnboarding()).toBe("reset");
  };

  try {
    await render("first run");
    expect(guide()?.dataset.folderGuide).toBe("sidebar");
    await click("sidebar");
    expect(guide()?.dataset.folderGuide).toBe("folder");
    expect(guide()?.textContent).toContain("In the desktop app, you can also drop a photo folder into the window.");
    vi.mocked(chooseFolder).mockResolvedValueOnce(null);
    await click("folder");
    expect(guide()).toBeTruthy();
    expect(loadFolderOnboarding()).toBe("new");

    await click("Skip guide");
    await render("after skip");
    expect(guide()).toBeNull();
    expect(host.querySelector(".workspace-empty")?.textContent).toContain("Choose folder");

    await resetGuide();
    expect(guide()?.dataset.folderGuide).toBe("folder");
    vi.mocked(chooseFolder).mockResolvedValue("/demo/Field Notes");
    vi.mocked(openFolder).mockRejectedValueOnce(new Error("Folder unavailable"));
    await click("folder");
    expect(guide()?.dataset.folderGuide).toBe("folder");
    expect(loadFolderOnboarding()).toBe("reset");

    await click("folder");
    expect(guide()?.dataset.folderGuide).toBe("grid");
    expect(loadFolderOnboarding()).toBe("grid");
    await click("grid");
    expect(useWorkspaceStore.getState().view).toBe("grid");
    await click("Next");
    expect(guide()?.dataset.folderGuide).toBe("loupe");
    await click("loupe");
    expect(useWorkspaceStore.getState().view).toBe("loupe");
    await click("Next");
    expect(guide()?.dataset.folderGuide).toBe("inspector");
    await click("Finish");
    expect(guide()).toBeNull();
    expect(loadFolderOnboarding()).toBe("done");
    const directory = host.querySelector(".statusbar__asset-directory")?.textContent;
    expect(directory).toMatch(/^\/demo\/Field Notes\/?$/);

    await resetGuide();
    await render("after reset");
    expect(guide()?.dataset.folderGuide).toBe("folder");
    await click("Next");
    expect(guide()?.dataset.folderGuide).toBe("grid");
    expect(host.querySelector(".statusbar__asset-directory")?.textContent).toBe(directory);
  } finally {
    await act(async () => root.unmount());
    host.remove();
    client.clear();
    useWorkspaceStore.setState(initialState);
    localStorage.clear();
    userAgent.mockRestore();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  }
});
