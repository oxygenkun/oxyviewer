import { useEffect, useRef, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/Button";
import type { MessageKey } from "@/lib/i18n";
import styles from "./FolderGuide.module.css";

export type GuideStep = "folder" | "grid" | "loupe" | "inspector";
export type GuideAnchor = GuideStep | "sidebar";

interface FolderGuideProps {
  shellRef: RefObject<HTMLDivElement | null>;
  anchor: GuideAnchor;
  title: MessageKey;
  body: MessageKey;
  action?: { label: MessageKey; run: () => void };
  onSkip: () => void;
  t: (key: MessageKey) => string;
}

const MARGIN = 12;
const GAP = 10;

export function FolderGuide({ shellRef, anchor, title, body, action, onSkip, t }: FolderGuideProps) {
  const guideRef = useRef<HTMLElement>(null);

  useEffect(() => {
    const shell = shellRef.current;
    const guide = guideRef.current;
    if (!shell || !guide) return;
    const target = shell.querySelector<HTMLElement>(`[data-guide-anchor="${anchor}"]`);
    const fallback = anchor === "inspector"
      ? shell.querySelector<HTMLElement>('[data-guide-anchor="inspector-toggle"]') : null;
    const updatePosition = () => {
      const available = (element: HTMLElement | null) => {
        const rect = element?.getBoundingClientRect();
        return rect?.width && rect.height ? rect : undefined;
      };
      const primary = available(target);
      const rect = primary ?? available(fallback);
      const { width, height } = guide.getBoundingClientRect();
      const pointX = rect ? rect.left + rect.width / 2 : window.innerWidth - MARGIN;
      const besideInspector = anchor === "inspector" && primary && primary.left >= width + MARGIN + GAP;
      const left = besideInspector ? primary.left - width - GAP
        : Math.max(MARGIN, Math.min(pointX - width / 2, window.innerWidth - width - MARGIN));
      const aboveTarget = rect && rect.bottom + GAP + height > window.innerHeight - MARGIN && rect.top >= height + GAP + MARGIN;
      const preferredTop = besideInspector ? primary.top + 44
        : aboveTarget ? rect.top - height - GAP : (rect?.bottom ?? 42) + GAP;
      const top = Math.max(MARGIN, Math.min(preferredTop, window.innerHeight - height - MARGIN));
      const placement = besideInspector ? "left" : aboveTarget ? "above" : rect ? "below" : "unanchored";
      guide.dataset.placement = placement;
      guide.style.left = `${left}px`;
      guide.style.top = `${top}px`;
      guide.style.setProperty("--guide-arrow", `${Math.max(14, Math.min(pointX - left, width - 14))}px`);
    };

    updatePosition();
    const observer = new ResizeObserver(updatePosition);
    observer.observe(shell);
    observer.observe(guide);
    if (target) observer.observe(target);
    if (fallback) observer.observe(fallback);
    const workspace = shell.querySelector(".workspace");
    if (workspace) observer.observe(workspace);
    window.addEventListener("resize", updatePosition);
    window.addEventListener("scroll", updatePosition, true);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", updatePosition);
      window.removeEventListener("scroll", updatePosition, true);
    };
  }, [anchor, body, shellRef, t]);

  return createPortal(
    <section ref={guideRef} className={styles.guide} data-folder-guide={anchor} aria-labelledby="folder-guide-title">
      <div aria-live="polite">
        <h2 id="folder-guide-title">{t(title)}</h2>
        <p>{t(body)}</p>
      </div>
      <div className={styles.actions}>
        {anchor !== "inspector" ? <Button size="small" variant="ghost" onClick={onSkip}>{t("dismissFolderGuide")}</Button> : null}
        {action ? <Button size="small" variant="primary" onClick={action.run}>{t(action.label)}</Button> : null}
      </div>
    </section>, document.body,
  );
}
