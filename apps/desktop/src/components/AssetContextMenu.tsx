import { ChevronRight, Copy, ExternalLink, FolderOpen, Trash2 } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { platformFileManager } from "../lib/folderPaths";
import type { MessageKey } from "../lib/i18n";
import type { AssetSummary, ExternalAppSettings, FileDeletionMode } from "../types";

export interface AssetMenuTarget { asset: AssetSummary; x: number; y: number }
interface Props {
  target: AssetMenuTarget;
  settings?: ExternalAppSettings;
  settingsError?: boolean;
  deletionMode: FileDeletionMode;
  t: (key: MessageKey) => string;
  onDismiss: () => void;
  onOpen: (path: string, appId?: string) => void;
  onSettings: () => void;
  onCopy: (asset: AssetSummary, relative: boolean) => void;
  onReveal: (path: string) => void;
  onTrash: (asset: AssetSummary) => void;
}

const managerLabels = { finder: "openInFinder", windowsExplorer: "openInWindowsExplorer", generic: "openInFileManager" } as const;
const clamp = (value: number, size: number, limit: number) => Math.max(8, Math.min(value, limit - size - 8));

export function AssetContextMenu({ target, settings, settingsError, deletionMode, t, onDismiss, onOpen, onSettings, onCopy, onReveal, onTrash }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const main = useRef<HTMLDivElement>(null);
  const submenu = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const keyboardOpen = useRef(false);
  const [expanded, setExpanded] = useState(false);
  const [position, setPosition] = useState({ x: target.x, y: target.y });
  const [subPosition, setSubPosition] = useState({ x: target.x, y: target.y });
  const defaultApp = settings?.apps.find((app) => app.id === settings.defaultAppId);
  const label = (name: string) => t("externalOpenWithName").replace("{name}", name);
  const run = (action: () => void) => { onDismiss(); action(); };

  useLayoutEffect(() => {
    const rect = main.current!.getBoundingClientRect();
    setPosition({ x: clamp(target.x, rect.width, window.innerWidth), y: clamp(target.y, rect.height, window.innerHeight) });
  }, [target.x, target.y, defaultApp, settingsError]);

  useLayoutEffect(() => {
    if (!expanded) return;
    const menuRect = main.current!.getBoundingClientRect();
    const triggerRect = trigger.current!.getBoundingClientRect();
    const subRect = submenu.current!.getBoundingClientRect();
    const right = menuRect.right - 1;
    setSubPosition({ x: clamp(right + subRect.width <= window.innerWidth - 8 ? right : menuRect.left - subRect.width + 1, subRect.width, window.innerWidth), y: clamp(triggerRect.top, subRect.height, window.innerHeight) });
    if (keyboardOpen.current) {
      submenu.current?.querySelector<HTMLButtonElement>("button")?.focus();
      keyboardOpen.current = false;
    }
  }, [expanded, position, settings]);

  useEffect(() => {
    const previous = document.activeElement;
    main.current?.querySelector<HTMLButtonElement>("button")?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus({ preventScroll: true }); };
  }, []);

  useEffect(() => {
    const outside = (event: Event) => { if (event.target instanceof Node && !root.current?.contains(event.target)) onDismiss(); };
    window.addEventListener("pointerdown", outside);
    window.addEventListener("blur", onDismiss);
    window.addEventListener("resize", onDismiss);
    document.addEventListener("scroll", outside, true);
    return () => {
      window.removeEventListener("pointerdown", outside);
      window.removeEventListener("blur", onDismiss);
      window.removeEventListener("resize", onDismiss);
      document.removeEventListener("scroll", outside, true);
    };
  }, [onDismiss]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    // Keep Loupe's navigation shortcuts from running while a menu owns focus.
    event.stopPropagation();
    const inSubmenu = submenu.current?.contains(event.target as Node);
    if (event.key === "Escape" || (event.key === "ArrowLeft" && inSubmenu)) {
      event.preventDefault();
      if (expanded) { setExpanded(false); trigger.current?.focus(); } else onDismiss();
    } else if (event.key === "ArrowRight" && event.target === trigger.current) {
      event.preventDefault();
      if (expanded) submenu.current?.querySelector<HTMLButtonElement>("button")?.focus();
      else { keyboardOpen.current = true; setExpanded(true); }
    } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const menu = inSubmenu ? submenu.current : main.current;
      const items = [...(menu?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [])];
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    } else if (event.key === "Tab") { onDismiss(); }
  };
  const closeSubmenu = () => setExpanded(false);
  return createPortal(<div ref={root} onKeyDown={onKeyDown} onContextMenu={(event) => event.preventDefault()}>
    <div ref={main} className="asset-context-menu asset-context-menu--external" role="menu" aria-label={target.asset.name} style={{ left: position.x, top: position.y }}>
      {defaultApp ? <button role="menuitem" onPointerEnter={closeSubmenu} onClick={() => run(() => onOpen(target.asset.path, defaultApp.id))}><ExternalLink size={13} /><span>{label(defaultApp.name)}</span></button> : null}
      <button ref={trigger} role="menuitem" aria-haspopup="menu" aria-expanded={expanded} onPointerEnter={() => setExpanded(true)}
        onClick={() => {
          if (expanded) submenu.current?.querySelector<HTMLButtonElement>("button")?.focus();
          else { keyboardOpen.current = true; setExpanded(true); }
        }}>
        <ExternalLink size={13} /><span>{t("externalOpenWith")}</span><ChevronRight size={13} />
      </button>
      <div className="asset-context-menu__separator" />
      <button role="menuitem" onPointerEnter={closeSubmenu} onClick={() => run(() => onCopy(target.asset, true))}><Copy size={13} />{t("copyRelativePath")}</button>
      <button role="menuitem" onPointerEnter={closeSubmenu} onClick={() => run(() => onCopy(target.asset, false))}><Copy size={13} />{t("copyAbsolutePath")}</button>
      <button role="menuitem" onPointerEnter={closeSubmenu} onClick={() => run(() => onReveal(target.asset.path))}><FolderOpen size={13} />{t(managerLabels[platformFileManager()])}</button>
      <div className="asset-context-menu__separator" />
      <button role="menuitem" className="asset-context-menu__danger" onPointerEnter={closeSubmenu} onClick={() => run(() => onTrash(target.asset))}><Trash2 size={13} />{t(deletionMode === "permanent" ? "deletePermanently" : "delete")}</button>
    </div>
    {expanded ? <div ref={submenu} className="asset-context-menu asset-context-menu--external" role="menu" aria-label={t("externalOpenWith")} style={{ left: subPosition.x, top: subPosition.y }}>
      {settings?.apps.map((app) => <button key={app.id} role="menuitem" title={app.executablePath} onClick={() => run(() => onOpen(target.asset.path, app.id))}><ExternalLink size={13} /><span>{label(app.name)}</span></button>)}
      {settings?.apps.length ? <div className="asset-context-menu__separator" /> : null}
      <button role="menuitem" onClick={() => run(() => onOpen(target.asset.path))}>{t("externalMore")}</button>
      {!settings?.apps.length || settingsError ? <button role="menuitem" onClick={() => run(onSettings)}>{t("externalConfigure")}</button> : null}
    </div> : null}
  </div>, document.body);
}
