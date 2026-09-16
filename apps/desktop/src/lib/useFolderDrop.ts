import { useEffect, useRef, useState } from "react";
import { isTauri, onWindowDragDrop } from "./api";
import { folderName } from "./folderImport";

export interface FolderDropState {
  visible: boolean;
  /** Preview names, capped so the overlay never grows without bound. */
  folderNames: string[];
  itemCount: number;
}

const HIDDEN_DROP: FolderDropState = { visible: false, folderNames: [], itemCount: 0 };
const PREVIEW_LIMIT = 4;

/**
 * Native OS drag-and-drop for the desktop window.
 *
 * Tauri intercepts the platform drop, so the dropped paths only arrive through
 * `onDragDropEvent`. `enter` and `leave` toggle the affordance; `over` repeats at
 * pointer frequency and is deliberately ignored, because the card covers the
 * window and has nothing to track. Nothing here may reach React state on `over`.
 *
 * The browser demo (`pnpm dev`) has no such channel, so it falls back to HTML5
 * events and infers folder names from `DataTransferItem`; that preview exists
 * solely to exercise the visuals and must not be read as import behavior.
 */
export function useFolderDrop(onDrop: (paths: string[]) => void) {
  const [dropState, setDropState] = useState<FolderDropState>(HIDDEN_DROP);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;

  useEffect(() => {
    if (!isTauri()) return;
    let dispose: (() => void) | undefined;
    let cancelled = false;
    void onWindowDragDrop((event) => {
      if (event.type === "enter") {
        setDropState({
          visible: true,
          folderNames: event.paths.slice(0, PREVIEW_LIMIT).map(folderName),
          itemCount: event.paths.length,
        });
        return;
      }
      if (event.type === "over") return;
      setDropState(HIDDEN_DROP);
      if (event.type === "drop") onDropRef.current(event.paths);
    }).then((unlisten) => {
      if (cancelled) unlisten();
      else dispose = unlisten;
    });
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  useEffect(() => {
    if (isTauri()) return;
    const preview = (event: DragEvent) => {
      const items = Array.from(event.dataTransfer?.items ?? []);
      return {
        names: items
          .map((item) => entryName(item))
          .filter((name): name is string => Boolean(name)),
        count: items.length || event.dataTransfer?.files.length || 0,
      };
    };
    const onDragEnter = (event: DragEvent) => {
      event.preventDefault();
      const { names, count } = preview(event);
      setDropState({ visible: true, folderNames: names.slice(0, PREVIEW_LIMIT), itemCount: count });
    };
    const onDragOver = (event: DragEvent) => {
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    };
    const onDragLeave = (event: DragEvent) => {
      if (event.relatedTarget === null) setDropState(HIDDEN_DROP);
    };
    const onDropEvent = (event: DragEvent) => {
      event.preventDefault();
      const { names } = preview(event);
      setDropState(HIDDEN_DROP);
      if (names.length) onDropRef.current(names.map((name) => `/demo/${name}`));
    };
    window.addEventListener("dragenter", onDragEnter);
    window.addEventListener("dragover", onDragOver);
    window.addEventListener("dragleave", onDragLeave);
    window.addEventListener("drop", onDropEvent);
    return () => {
      window.removeEventListener("dragenter", onDragEnter);
      window.removeEventListener("dragover", onDragOver);
      window.removeEventListener("dragleave", onDragLeave);
      window.removeEventListener("drop", onDropEvent);
    };
  }, []);

  return dropState;
}

function entryName(item: DataTransferItem): string | undefined {
  try {
    const entry = item.webkitGetAsEntry?.();
    if (entry) return entry.name;
  } catch {
    // The demo overlay is cosmetic; a blocked entry read must not break it.
  }
  return item.getAsFile()?.name;
}
