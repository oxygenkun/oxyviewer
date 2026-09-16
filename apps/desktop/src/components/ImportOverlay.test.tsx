// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ImportOverlay } from "./ImportOverlay";
import { translate } from "../lib/i18n";

let root: Root;
let container: HTMLDivElement;
const t = (key: Parameters<typeof translate>[1]) => translate("en", key);

function renderOverlay(props: { visible: boolean; folderNames: string[]; itemCount: number }) {
  return act(async () => root.render(<ImportOverlay {...props} t={t} />));
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

it("stays hidden and non-interactive until a drag enters the window", async () => {
  await renderOverlay({ visible: false, folderNames: [], itemCount: 0 });

  const overlay = container.querySelector(".drop-overlay")!;
  expect(overlay.classList.contains("is-visible")).toBe(false);
  expect(overlay.getAttribute("aria-hidden")).toBe("true");
});

it("previews the dropped folder names and folds the remainder into a count", async () => {
  await renderOverlay({ visible: true, folderNames: ["2024", "trips", "weddings", "raw"], itemCount: 6 });

  const overlay = container.querySelector(".drop-overlay")!;
  expect(overlay.classList.contains("is-visible")).toBe(true);
  expect(overlay.querySelector("strong")!.textContent).toBe(t("dropToImport"));
  expect(Array.from(overlay.querySelectorAll(".drop-overlay__names li")).map((item) => item.textContent))
    .toEqual(["2024", "trips", "weddings", "raw", "+2"]);
});
