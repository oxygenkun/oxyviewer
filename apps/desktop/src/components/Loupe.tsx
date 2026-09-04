import {
  Check,
  Focus,
  LocateFixed,
  Minus,
  Plus,
  Settings2,
} from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  clampPan,
  clampZoom,
  FILMSTRIP_GAP,
  filmstripItemWidth,
  filmstripUnloadedWidth,
  fitSize,
  getNavigatorViewport,
  MAX_PIXEL_ZOOM_PERCENT,
  panByNavigatorDelta,
  pixelZoomPercent,
  resolveLoupeSourceSize,
  zoomAtPoint,
  zoomForPixelPercent,
  type Point,
  type Size,
} from "../lib/loupe";
import { LAYOUT_SIZE_LIMITS } from "../lib/layoutSizing";
import { getAssetDetails } from "../lib/api";
import { mapFocusRegions } from "../lib/focusArea";
import type { MessageKey } from "../lib/i18n";
import type { RawPreviewStatus } from "../lib/rawPreview";
import { orderBySelectionPriority } from "../lib/selectionPriority";
import { renderPlan } from "../lib/preview";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, HeifDecodeStatus, NavigatorPosition } from "../types";
import { AssetMetadataBadges } from "./AssetMetadataBadges";
import { FilmstripPreviewPreloader } from "./FilmstripPreviewPreloader";
import { HeifTileCanvas } from "./HeifTileCanvas";
import { Thumbnail } from "./Thumbnail";
import { ResizeHandle } from "./ResizeHandle";

interface LoupeProps {
  assets: AssetSummary[];
  total: number;
  fetchNextPage: () => void;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  t: (key: MessageKey) => string;
}

const positions: NavigatorPosition[] = ["top-left", "top-right", "bottom-left", "bottom-right"];
const DEFAULT_IMAGE_SIZE: Size = { width: 3, height: 2 };
const NAVIGATOR_MAX_SIZE: Size = { width: 156, height: 98 };

function elementSize(element: HTMLElement | null): Size {
  return {
    width: element?.offsetWidth ?? 0,
    height: element?.offsetHeight ?? 0,
  };
}

function elementContentSize(element: HTMLElement): Size {
  const style = window.getComputedStyle(element);
  return {
    width: Math.max(0, element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)),
    height: Math.max(0, element.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom)),
  };
}

export function Loupe({
  assets,
  total,
  fetchNextPage,
  hasNextPage,
  isFetchingNextPage,
  t,
}: LoupeProps) {
  const activeId = useWorkspaceStore((state) => state.activeId);
  const select = useWorkspaceStore((state) => state.select);
  const navigatorVisible = useWorkspaceStore((state) => state.navigatorVisible);
  const navigatorPosition = useWorkspaceStore((state) => state.navigatorPosition);
  const hardwareAcceleration = useWorkspaceStore((state) => state.hardwareAcceleration);
  const displaySharpening = useWorkspaceStore((state) => state.displaySharpening);
  const focusAreasVisible = useWorkspaceStore((state) => state.focusAreasVisible);
  const loupeMetadataVisible = useWorkspaceStore((state) => state.loupeMetadataVisible);
  const loupeControlsAutoHide = useWorkspaceStore((state) => state.loupeControlsAutoHide);
  const filmstripHeight = useWorkspaceStore((state) => state.filmstripHeight);
  const setNavigatorVisible = useWorkspaceStore((state) => state.setNavigatorVisible);
  const setNavigatorPosition = useWorkspaceStore((state) => state.setNavigatorPosition);
  const setFocusAreasVisible = useWorkspaceStore((state) => state.setFocusAreasVisible);
  const setLoupeControlsAutoHide = useWorkspaceStore((state) => state.setLoupeControlsAutoHide);
  const setFilmstripHeight = useWorkspaceStore((state) => state.setFilmstripHeight);
  const active = assets.find((asset) => asset.id === activeId) ?? assets[0];
  const stageRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLDivElement>(null);
  const navigatorRef = useRef<HTMLDivElement>(null);
  const settingsRef = useRef<HTMLDivElement>(null);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const filmstripRef = useRef<HTMLDivElement>(null);
  const loupeRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; start: Point; offset: Point } | undefined>(undefined);
  const navigatorDragRef = useRef<{ pointerId: number; last: Point } | undefined>(undefined);
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState<Point>({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [focusTemporarilyInverted, setFocusTemporarilyInverted] = useState(false);
  const [editingZoom, setEditingZoom] = useState(false);
  const [zoomInput, setZoomInput] = useState("");
  const [hideControlsImmediately, setHideControlsImmediately] = useState(false);
  const [layoutVersion, setLayoutVersion] = useState(0);
  const [stageContentSize, setStageContentSize] = useState<Size>({ width: 0, height: 0 });
  const [naturalSize, setNaturalSize] = useState<{ assetId: string; size: Size } | undefined>(undefined);
  const [heifFullSize, setHeifFullSize] = useState<{ assetId: string; size: Size } | undefined>(undefined);
  const [rawPreviewStatus, setRawPreviewStatus] = useState<RawPreviewStatus>({ state: "loadingPreview" });
  const [heifStatus, setHeifStatus] = useState<HeifDecodeStatus>("probing");
  const [visibleFilmstripIds, setVisibleFilmstripIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const unloadedFilmstripCount = Math.max(0, total - assets.length);
  const unloadedFilmstripWidth = filmstripUnloadedWidth(
    unloadedFilmstripCount,
    filmstripHeight,
  );
  const details = useQuery({
    queryKey: ["asset-details", active.id],
    queryFn: () => getAssetDetails(active),
  });
  const heifUsesTiles = active.kind === "heif"
    && renderPlan(active.kind, "loupe").some(
      (step) => step.level === "full" && step.method.type === "heifTiles",
    );

  const metadataSize = details.data?.width && details.data.height
    ? { width: details.data.width, height: details.data.height }
    : undefined;
  const sourceSize = resolveLoupeSourceSize(
    active.kind,
    naturalSize?.assetId === active.id ? naturalSize.size : undefined,
    heifFullSize?.assetId === active.id ? heifFullSize.size : undefined,
    metadataSize,
    DEFAULT_IMAGE_SIZE,
  );
  const fittedImageSize = fitSize(stageContentSize, sourceSize);
  const navigatorImageSize = fitSize(NAVIGATOR_MAX_SIZE, sourceSize);
  const currentNaturalSize = naturalSize?.assetId === active.id ? naturalSize.size : undefined;
  const currentHeifSize = heifFullSize?.assetId === active.id ? heifFullSize.size : undefined;
  const displayedNaturalSize = active.kind === "heif"
    ? currentHeifSize ?? currentNaturalSize ?? metadataSize
    : currentNaturalSize ?? metadataSize;
  const mappedFocusRegions = useMemo(
    () => mapFocusRegions(details.data?.focusInfo, displayedNaturalSize, metadataSize),
    [details.data?.focusInfo, displayedNaturalSize, metadataSize],
  );
  const showFocusAreas = focusAreasVisible !== focusTemporarilyInverted;

  useEffect(() => {
    if (activeId !== active.id) select(active.id);
  }, [active.id, activeId, select]);

  const getSizes = useCallback(() => ({
    stage: elementSize(stageRef.current),
    image: elementSize(imageRef.current),
  }), []);

  const setZoomAroundPoint = useCallback((next: number, pointFromStageCenter: Point) => {
    const maxZoom = zoomForPixelPercent(fittedImageSize, sourceSize, MAX_PIXEL_ZOOM_PERCENT);
    const nextZoom = clampZoom(next, maxZoom);
    const { stage, image } = getSizes();
    setOffset((current) => zoomAtPoint(zoom, nextZoom, current, pointFromStageCenter, stage, image));
    setZoom(nextZoom);
  }, [fittedImageSize, getSizes, sourceSize, zoom]);

  const resetZoom = useCallback(() => {
    setZoom(1);
    setOffset({ x: 0, y: 0 });
  }, []);

  const handleHeifImageSize = useCallback((size: Size) => {
    setHeifFullSize((current) => (
      current?.assetId === active.id
        && current.size.width === size.width
        && current.size.height === size.height
        ? current
        : { assetId: active.id, size }
    ));
  }, [active.id]);

  const handleHeifPreviewStatus = useCallback((status: RawPreviewStatus) => {
    setHeifStatus(status.state === "fullReady"
      ? "complete"
      : status.state === "fullFailed"
        ? "failed"
        : "decoding");
  }, []);

  const handleImageLoad = useCallback((size: Size) => {
    setNaturalSize({ assetId: active.id, size });
  }, [active.id]);

  useEffect(() => {
    resetZoom();
    setRawPreviewStatus({ state: "loadingPreview" });
    setHeifStatus("probing");
  }, [active.id, resetZoom]);

  useEffect(() => {
    if (!settingsOpen) return;
    const closeSettingsOutside = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (settingsRef.current?.contains(target) || settingsButtonRef.current?.contains(target)) return;
      setSettingsOpen(false);
    };
    document.addEventListener("pointerdown", closeSettingsOutside);
    return () => document.removeEventListener("pointerdown", closeSettingsOutside);
  }, [settingsOpen]);

  useEffect(() => {
    const editableTarget = (target: EventTarget | null) => (
      target instanceof HTMLElement
      && Boolean(target.closest("input, textarea, select, [contenteditable='true']"))
    );
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Alt") {
        if (!event.repeat) setFocusTemporarilyInverted(true);
        return;
      }
      if (
        event.key.toLowerCase() === "f"
        && !event.repeat
        && !event.altKey
        && !event.ctrlKey
        && !event.metaKey
        && !editableTarget(event.target)
      ) {
        event.preventDefault();
        setFocusAreasVisible(!focusAreasVisible);
      }
    };
    const handleKeyUp = (event: KeyboardEvent) => {
      if (event.key === "Alt") setFocusTemporarilyInverted(false);
    };
    const resetTemporaryState = () => setFocusTemporarilyInverted(false);
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    window.addEventListener("blur", resetTemporaryState);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
      window.removeEventListener("blur", resetTemporaryState);
    };
  }, [focusAreasVisible, setFocusAreasVisible]);

  useLayoutEffect(() => {
    const stage = stageRef.current;
    const image = imageRef.current;
    if (!stage || !image) return;
    const observer = new ResizeObserver(() => {
      const sizes = getSizes();
      const contentSize = elementContentSize(stage);
      setStageContentSize((current) => (
        current.width === contentSize.width && current.height === contentSize.height
          ? current
          : contentSize
      ));
      setOffset((current) => clampPan(current, zoom, sizes.stage, sizes.image));
      setLayoutVersion((version) => version + 1);
    });
    observer.observe(stage);
    observer.observe(image);
    return () => observer.disconnect();
  }, [getSizes, zoom]);

  useEffect(() => {
    const strip = filmstripRef.current;
    if (!strip) return;
    const items = Array.from(
      strip.querySelectorAll<HTMLButtonElement>("button[data-filmstrip-asset-id]"),
    );
    if (typeof IntersectionObserver === "undefined") {
      setVisibleFilmstripIds(new Set(items.map((item) => item.dataset.filmstripAssetId!)));
      return;
    }

    const observer = new IntersectionObserver((entries) => {
      setVisibleFilmstripIds((current) => {
        const next = new Set(current);
        for (const entry of entries) {
          const id = (entry.target as HTMLButtonElement).dataset.filmstripAssetId;
          if (!id) continue;
          if (entry.isIntersecting) next.add(id);
          else next.delete(id);
        }
        if (next.size === current.size && [...next].every((id) => current.has(id))) return current;
        return next;
      });
    }, { root: strip });
    items.forEach((item) => observer.observe(item));
    return () => observer.disconnect();
  }, [assets]);

  const priorityOrderedAssets = useMemo(
    () => orderBySelectionPriority(assets, active.id, (asset) => asset.id),
    [active.id, assets],
  );
  const priorityRankById = useMemo(
    () => new Map(priorityOrderedAssets.map((asset, index) => [asset.id, index])),
    [priorityOrderedAssets],
  );
  const visibleFilmstripAssets = useMemo(
    () => priorityOrderedAssets.filter(
      (asset) => asset.id === active.id || visibleFilmstripIds.has(asset.id),
    ),
    [active.id, priorityOrderedAssets, visibleFilmstripIds],
  );

  const fetchFilmstripPageIfNeeded = useCallback((strip: HTMLDivElement) => {
    if (!hasNextPage || isFetchingNextPage) return;
    const loadedRight = 9 + assets.length * (filmstripItemWidth(filmstripHeight) + FILMSTRIP_GAP);
    if (strip.scrollLeft + strip.clientWidth >= loadedRight - 400) fetchNextPage();
  }, [assets.length, fetchNextPage, filmstripHeight, hasNextPage, isFetchingNextPage]);

  useEffect(() => {
    const strip = filmstripRef.current;
    if (strip) fetchFilmstripPageIfNeeded(strip);
  }, [fetchFilmstripPageIfNeeded]);

  const handleWheel = useCallback((event: React.WheelEvent<HTMLDivElement>) => {
    if ((event.target as HTMLElement).closest(".loupe__controls, .loupe__navigator, .loupe__settings")) return;
    event.preventDefault();
    const rect = stageRef.current?.getBoundingClientRect();
    if (!rect) return;
    const point = {
      x: event.clientX - rect.left - rect.width / 2,
      y: event.clientY - rect.top - rect.height / 2,
    };
    setZoomAroundPoint(zoom * Math.exp(-event.deltaY * 0.002), point);
  }, [setZoomAroundPoint, zoom]);

  const handlePointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const overlay = (event.target as HTMLElement)
      .closest(".loupe__controls, .loupe__navigator, .loupe__settings");
    if (zoom <= 1 || event.button !== 0 || overlay) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      start: { x: event.clientX, y: event.clientY },
      offset,
    };
    setDragging(true);
  }, [offset, zoom]);

  const handlePointerMove = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const { stage, image } = getSizes();
    setOffset(clampPan({
      x: drag.offset.x + event.clientX - drag.start.x,
      y: drag.offset.y + event.clientY - drag.start.y,
    }, zoom, stage, image));
  }, [getSizes, zoom]);

  const finishDrag = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (dragRef.current?.pointerId !== event.pointerId) return;
    dragRef.current = undefined;
    setDragging(false);
  }, []);

  const dragNavigator = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const drag = navigatorDragRef.current;
    const rect = navigatorRef.current?.getBoundingClientRect();
    if (!drag || drag.pointerId !== event.pointerId || !rect) return;
    const pointerDelta = {
      x: event.clientX - drag.last.x,
      y: event.clientY - drag.last.y,
    };
    drag.last = { x: event.clientX, y: event.clientY };
    const { stage, image } = getSizes();
    setOffset((current) => panByNavigatorDelta(
      current,
      pointerDelta,
      zoom,
      stage,
      image,
      { width: rect.width, height: rect.height },
    ));
  }, [getSizes, zoom]);

  const navigatorViewport = useMemo(() => {
    const { stage, image } = getSizes();
    if (!stage.width || !image.width) return { left: 0, top: 0, width: 1, height: 1 };
    return getNavigatorViewport(zoom, offset, stage, image);
  }, [getSizes, layoutVersion, offset, zoom]);

  const zoomLabel = `${pixelZoomPercent(fittedImageSize, sourceSize, zoom)}%`;
  const beginZoomEdit = () => {
    setZoomInput(String(pixelZoomPercent(fittedImageSize, sourceSize, zoom)));
    setEditingZoom(true);
  };
  const commitZoomEdit = () => {
    const percent = Number(zoomInput);
    if (zoomInput.trim() && Number.isFinite(percent)) {
      setZoomAroundPoint(
        zoomForPixelPercent(fittedImageSize, sourceSize, percent),
        { x: 0, y: 0 },
      );
    }
    setEditingZoom(false);
  };
  const positionLabels: Record<NavigatorPosition, MessageKey> = {
    "top-left": "topLeft",
    "top-right": "topRight",
    "bottom-left": "bottomLeft",
    "bottom-right": "bottomRight",
  };

  return (
    <div
      className="loupe"
      ref={loupeRef}
      style={{ "--filmstrip-height": `${filmstripHeight}px` } as React.CSSProperties}
    >
      <FilmstripPreviewPreloader assets={visibleFilmstripAssets} />
      <div
        className={`loupe__stage ${zoom > 1 ? "is-zoomed" : ""} ${dragging ? "is-dragging" : ""}`}
        ref={stageRef}
        onWheel={handleWheel}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={finishDrag}
        onPointerCancel={finishDrag}
      >
        <div className="loupe__caption">
          <span>{active.name}</span>
          <small>{active.extension} · {formatBytes(active.sizeBytes)}</small>
          {loupeMetadataVisible ? <AssetMetadataBadges asset={active} /> : null}
        </div>
        {active.kind === "raw" ? (
          <div className={`loupe__raw-status loupe__raw-status--${rawPreviewStatus.state}`}>
            <i />
            {rawPreviewStatus.state === "loadingPreview"
              ? t("rawLoadingPreview")
              : rawPreviewStatus.state === "developingFull"
                ? t("rawDevelopingFull")
                : rawPreviewStatus.state === "fullFailed"
                  ? t("rawFullFailed")
                  : `${t("rawFullReady")} · ${rawPreviewStatus.width} × ${rawPreviewStatus.height}`}
          </div>
        ) : active.kind === "heif" ? (
          <div className={`loupe__raw-status loupe__raw-status--${heifStatus}`}>
            <i />
            {heifStatus === "complete"
              ? t("fullQualityReady")
              : heifStatus === "compatibilityFallback"
                ? t("fullQualityCompatibility")
                : heifStatus === "failed"
                  ? t("fullQualityFailed")
                  : t("fullQualityLoading")}
          </div>
        ) : null}
        <div
          className="loupe__image"
          ref={imageRef}
          style={{
            width: fittedImageSize.width || undefined,
            height: fittedImageSize.height || undefined,
            transform: `translate3d(${offset.x}px, ${offset.y}px, 0)`,
          }}
        >
          <div
            className="loupe__render"
            style={{
              width: fittedImageSize.width ? fittedImageSize.width * zoom : undefined,
              height: fittedImageSize.height ? fittedImageSize.height * zoom : undefined,
            }}
          >
            <Thumbnail
              asset={active}
              large
              onImageLoad={handleImageLoad}
              onRawPreviewStatus={active.kind === "heif"
                ? handleHeifPreviewStatus
                : setRawPreviewStatus}
            />
            {heifUsesTiles ? (
              <HeifTileCanvas
                key={active.id}
                asset={active}
                displaySharpening={displaySharpening}
                hardwareAcceleration={hardwareAcceleration}
                onImageSize={handleHeifImageSize}
                onStatus={setHeifStatus}
              />
            ) : null}
            {showFocusAreas && mappedFocusRegions.length > 0 ? (
              <div className="loupe__focus-overlay" aria-hidden="true">
                {mappedFocusRegions.map((region, index) => (
                  <i
                    key={index}
                    className={`loupe__focus-frame ${region.syntheticFrame ? "is-estimated" : ""}`}
                  style={{
                    left: `${region.left * 100}%`,
                    top: `${region.top * 100}%`,
                    width: `${region.width * 100}%`,
                    height: `${region.height * 100}%`,
                  }}
                  />
                ))}
              </div>
            ) : null}
          </div>
        </div>
        {navigatorVisible && zoom > 1.001 ? (
          <div className={`loupe__navigator loupe__navigator--${navigatorPosition}`}>
            <div
              className="loupe__navigator-image"
              ref={navigatorRef}
              style={{
                width: navigatorImageSize.width || undefined,
                height: navigatorImageSize.height || undefined,
              }}
              onPointerDown={(event) => {
                if (event.button !== 0) return;
                event.currentTarget.setPointerCapture(event.pointerId);
                navigatorDragRef.current = {
                  pointerId: event.pointerId,
                  last: { x: event.clientX, y: event.clientY },
                };
              }}
              onPointerMove={dragNavigator}
              onPointerUp={(event) => {
                if (navigatorDragRef.current?.pointerId === event.pointerId) navigatorDragRef.current = undefined;
              }}
              onPointerCancel={() => {
                navigatorDragRef.current = undefined;
              }}
            >
              <Thumbnail asset={active} large />
              <i
                className="loupe__navigator-viewport"
                style={{
                  left: `${navigatorViewport.left * 100}%`,
                  top: `${navigatorViewport.top * 100}%`,
                  width: `${navigatorViewport.width * 100}%`,
                  height: `${navigatorViewport.height * 100}%`,
                }}
              />
            </div>
            <span><LocateFixed size={11} /> {zoomLabel}</span>
          </div>
        ) : null}
        <div
          className={`loupe__controls ${loupeControlsAutoHide ? "is-auto-hidden" : ""} ${hideControlsImmediately ? "is-hide-immediate" : ""} ${settingsOpen ? "is-settings-open" : ""}`}
          onPointerEnter={() => setHideControlsImmediately(false)}
          onPointerLeave={(event) => {
            const opacity = Number.parseFloat(window.getComputedStyle(event.currentTarget).opacity);
            setHideControlsImmediately(opacity < 0.999);
          }}
        >
          <button onClick={() => setZoomAroundPoint(zoom / 1.25, { x: 0, y: 0 })} title={t("zoomOut")}>
            <Minus size={14} />
          </button>
          {editingZoom ? (
            <label className="loupe__zoom-input">
              <input
                autoFocus
                inputMode="numeric"
                min={pixelZoomPercent(fittedImageSize, sourceSize, 1)}
                max={MAX_PIXEL_ZOOM_PERCENT}
                step="1"
                type="number"
                value={zoomInput}
                onBlur={commitZoomEdit}
                onChange={(event) => setZoomInput(event.target.value)}
                onFocus={(event) => event.currentTarget.select()}
                onKeyDown={(event) => {
                  if (event.key === "Enter") event.currentTarget.blur();
                  if (event.key === "Escape") setEditingZoom(false);
                }}
              />
              <span>%</span>
            </label>
          ) : (
            <button className="loupe__zoom-label" onClick={beginZoomEdit} title={t("setZoom")}>
              {zoomLabel}
            </button>
          )}
          <button onClick={() => setZoomAroundPoint(zoom * 1.25, { x: 0, y: 0 })} title={t("zoomIn")}>
            <Plus size={14} />
          </button>
          <i />
          <button
            aria-keyshortcuts="F"
            aria-pressed={showFocusAreas}
            className={showFocusAreas ? "is-active" : ""}
            onClick={() => setFocusAreasVisible(!focusAreasVisible)}
            title={`${showFocusAreas ? t("hideFocusAreas") : t("showFocusAreas")} · ${t("focusShortcutHint")}`}
          >
            <Focus size={14} />
          </button>
          <button
            ref={settingsButtonRef}
            className={settingsOpen ? "is-active" : ""}
            onClick={() => setSettingsOpen((open) => !open)}
            title={t("loupeSettings")}
          >
            <Settings2 size={14} />
          </button>
        </div>
        {settingsOpen ? (
          <div className="loupe__settings" ref={settingsRef}>
            <header>
              <span>{t("loupeSettings")}</span>
              <small>{t("wheelZoomHint")}</small>
            </header>
             <button
              aria-pressed={loupeControlsAutoHide}
              className="loupe__settings-toggle"
              onClick={() => setLoupeControlsAutoHide(!loupeControlsAutoHide)}
            >
              <span>{t("autoHideToolbar")}</span>
              <i className={loupeControlsAutoHide ? "is-on" : ""}><b /></i>
            </button>
            <button className="loupe__settings-toggle" onClick={() => setNavigatorVisible(!navigatorVisible)}>
              <span>{t("showNavigator")}</span>
              <i className={navigatorVisible ? "is-on" : ""}><b /></i>
            </button>
            <span className="loupe__settings-label">{t("navigatorPosition")}</span>
            <div className="loupe__position-grid">
              {positions.map((position) => (
                <button
                  key={position}
                  className={navigatorPosition === position ? "is-active" : ""}
                  onClick={() => setNavigatorPosition(position)}
                  title={t(positionLabels[position])}
                >
                  <span className={`loupe__position-dot loupe__position-dot--${position}`} />
                  {navigatorPosition === position ? <Check size={10} /> : null}
                </button>
              ))}
            </div>
          </div>
        ) : null}
      </div>
      <ResizeHandle
        axis="y"
        className="resize-handle--filmstrip"
        cssVariable="--filmstrip-height"
        defaultValue={LAYOUT_SIZE_LIMITS.filmstrip.defaultValue}
        direction={-1}
        label={t("resizeFilmstrip")}
        max={LAYOUT_SIZE_LIMITS.filmstrip.max}
        min={LAYOUT_SIZE_LIMITS.filmstrip.min}
        onCommit={setFilmstripHeight}
        targetRef={loupeRef}
        value={filmstripHeight}
      />
      <div
        className="filmstrip"
        ref={filmstripRef}
        onScroll={(event) => fetchFilmstripPageIfNeeded(event.currentTarget)}
        onWheel={(event) => {
          if (event.deltaY === 0) return;
          event.preventDefault();
          event.currentTarget.scrollLeft += event.deltaX + event.deltaY;
        }}
      >
        {assets.map((asset) => (
          <FilmstripItem
            key={asset.id}
            active={active.id === asset.id}
            asset={asset}
            onClick={() => select(asset.id)}
            queueOrder={priorityRankById.get(asset.id) ?? assets.length}
            root={filmstripRef}
            showMetadata={loupeMetadataVisible}
          />
        ))}
        {unloadedFilmstripCount > 0 ? (
          <span
            aria-hidden="true"
            className="filmstrip__unloaded"
            style={{ flexBasis: unloadedFilmstripWidth }}
          />
        ) : null}
        {isFetchingNextPage ? <span className="filmstrip__loading">Loading...</span> : null}
      </div>
    </div>
  );
}

interface FilmstripItemProps {
  active: boolean;
  asset: AssetSummary;
  onClick: () => void;
  queueOrder: number;
  root: React.RefObject<HTMLDivElement | null>;
  showMetadata: boolean;
}

function FilmstripItem({
  active,
  asset,
  onClick,
  queueOrder,
  root,
  showMetadata,
}: FilmstripItemProps) {
  const itemRef = useRef<HTMLButtonElement>(null);
  const [nearby, setNearby] = useState(active);
  const [visible, setVisible] = useState(active);

  useEffect(() => {
    const item = itemRef.current;
    if (!item || !root.current || typeof IntersectionObserver === "undefined") {
      setNearby(true);
      setVisible(true);
      return;
    }
    const nearbyObserver = new IntersectionObserver(
      ([entry]) => setNearby(entry.isIntersecting),
      { root: root.current, rootMargin: "0px 320px" },
    );
    const visibleObserver = new IntersectionObserver(
      ([entry]) => setVisible(entry.isIntersecting),
      { root: root.current },
    );
    nearbyObserver.observe(item);
    visibleObserver.observe(item);
    return () => {
      nearbyObserver.disconnect();
      visibleObserver.disconnect();
    };
  }, [root]);

  useEffect(() => {
    if (active) {
      setNearby(true);
      setVisible(true);
      itemRef.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }, [active]);

  return (
    <button
      ref={itemRef}
      data-filmstrip-asset-id={asset.id}
      className={active ? "is-active" : ""}
      onClick={onClick}
      title={asset.name}
    >
      <Thumbnail
        asset={asset}
        enabled={nearby || active}
        priority={active ? "loupe" : visible ? "visible" : "nearby"}
        queueOrder={queueOrder}
      />
      {showMetadata ? (
        <span className="filmstrip__metadata">
          <AssetMetadataBadges asset={asset} />
        </span>
      ) : null}
      <span className="filmstrip__name">{asset.name}</span>
    </button>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}
