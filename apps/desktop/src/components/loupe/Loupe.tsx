import { RegionOverlay } from "@/components/analyzers/RegionOverlay";
import type { DisplayedPreviewSize } from "@/lib/preview/previewGeometry";
import { RawDecoderPanel } from "@/components/settings/RawDecoderPanel";
import {
  Check,
  Focus,
  ScanFace,
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
  fitSize,
  getNavigatorViewport,
  MAX_PIXEL_ZOOM_PERCENT,
  nextCycleZoom,
  panByNavigatorDelta,
  panFromNavigatorPoint,
  pixelZoomPercent,
  resolveLoupeSourceSize,
  zoomAtPoint,
  zoomForPixelPercent,
  type Point,
  type Size,
} from "@/lib/preview/loupe";
import { LAYOUT_SIZE_LIMITS } from "@/lib/ui/layoutSizing";
import { matchesAction } from "@/lib/ui/shortcuts";
import { useMarkingShortcuts } from "@/lib/hooks/useMarkingShortcuts";
import { getAssetDetails } from "@/lib/api";
import { focusRegionAnchor, mapFocusRegions } from "@/lib/ui/focusArea";
import { FaceOverlay } from "@/components/loupe/FaceOverlay";
import type { MessageKey } from "@/lib/i18n";
import type { RawPreviewStatus } from "@/lib/media/rawPreview";
import { renderPlan } from "@/lib/preview/preview";
import { useWorkspaceStore } from "@/store";
import type { AssetSummary, HeifDecodeStatus, NavigatorPosition } from "@/types";
import { AssetMetadataBadges } from "@/components/common/AssetMetadataBadges";
import { Filmstrip } from "@/components/browsing/Filmstrip";
import { HeifTileCanvas } from "./HeifTileCanvas";
import { Thumbnail } from "@/components/browsing/Thumbnail";
import { ResizeHandle } from "@/components/browsing/ResizeHandle";

interface LoupeProps {
  assets: AssetSummary[];
  restoringActiveId?: string;
  total: number;
  fetchNextPage: () => void;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  keyboardSuppressed?: boolean;
  onAssetContextMenu: (event: React.MouseEvent, asset: AssetSummary) => void;
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
  restoringActiveId,
  total,
  fetchNextPage,
  hasNextPage,
  isFetchingNextPage,
  keyboardSuppressed = false,
  onAssetContextMenu,
  t,
}: LoupeProps) {
  const activeId = useWorkspaceStore((state) => state.activeId);
  const select = useWorkspaceStore((state) => state.select);
  const navigatorVisible = useWorkspaceStore((state) => state.navigatorVisible);
  const navigatorPosition = useWorkspaceStore((state) => state.navigatorPosition);
  const displaySharpening = useWorkspaceStore((state) => state.displaySharpening);
  const focusAreasVisible = useWorkspaceStore((state) => state.focusAreasVisible);
  const faceBoxesVisible = useWorkspaceStore((state) => state.faceBoxesVisible);
  const loupeMetadataVisible = useWorkspaceStore((state) => state.loupeMetadataVisible);
  const loupeControlsAutoHide = useWorkspaceStore((state) => state.loupeControlsAutoHide);
  const filmstripHeight = useWorkspaceStore((state) => state.filmstripHeight);
  const setNavigatorVisible = useWorkspaceStore((state) => state.setNavigatorVisible);
  const setNavigatorPosition = useWorkspaceStore((state) => state.setNavigatorPosition);
  const setFocusAreasVisible = useWorkspaceStore((state) => state.setFocusAreasVisible);
  const setFaceBoxesVisible = useWorkspaceStore((state) => state.setFaceBoxesVisible);
  const setLoupeControlsAutoHide = useWorkspaceStore((state) => state.setLoupeControlsAutoHide);
  const setFilmstripHeight = useWorkspaceStore((state) => state.setFilmstripHeight);
  const shortcuts = useWorkspaceStore((state) => state.shortcuts);
  const active = assets.find((asset) => asset.id === activeId) ?? assets[0];
  const stageRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLDivElement>(null);
  const navigatorRef = useRef<HTMLDivElement>(null);
  const settingsRef = useRef<HTMLDivElement>(null);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
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
  const [naturalSize, setNaturalSize] = useState<{ assetId: string; size: DisplayedPreviewSize } | undefined>(undefined);
  const [heifFullSize, setHeifFullSize] = useState<{ assetId: string; size: Size } | undefined>(undefined);
  const [rawPreviewStatus, setRawPreviewStatus] = useState<RawPreviewStatus>({ state: "loadingPreview" });
  const [displayedHeifArtifact, setDisplayedHeifArtifact] = useState<string>();
  const heifPresentationIdentity = `${active.id}:${active.modifiedAtMs}:${displaySharpening}`;
  const handleHeifArtifactDisplayed = useCallback(() => setDisplayedHeifArtifact(heifPresentationIdentity), [heifPresentationIdentity]);
  const [heifStatus, setHeifStatus] = useState<HeifDecodeStatus | "probing">("probing");
  const details = useQuery({
    queryKey: ["asset-details", active.id],
    queryFn: () => getAssetDetails(active),
  });
  const heifUsesFullPresentation = active.kind === "heif"
    && renderPlan(active.kind, "loupe").some(
      (step) => step.level === "full" && step.method.type === "heifFull",
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
    naturalSize?.assetId === active.id ? naturalSize.size.geometry : undefined,
  );
  const fittedImageSize = fitSize(stageContentSize, sourceSize);
  const navigatorImageSize = fitSize(NAVIGATOR_MAX_SIZE, sourceSize);
  const currentNaturalSize = naturalSize?.assetId === active.id ? naturalSize.size : undefined;
  const currentHeifSize = heifFullSize?.assetId === active.id ? heifFullSize.size : undefined;
  const displayedNaturalSize = active.kind === "heif"
    ? currentHeifSize ?? currentNaturalSize?.geometry?.displaySize ?? currentNaturalSize ?? metadataSize
    : currentNaturalSize ?? metadataSize;
  const mappedFocusRegions = useMemo(
    () => mapFocusRegions(details.data?.focusInfo, displayedNaturalSize, metadataSize),
    [details.data?.focusInfo, displayedNaturalSize, metadataSize],
  );
  const showFocusAreas = focusAreasVisible !== focusTemporarilyInverted;

  useEffect(() => {
    if (activeId !== active.id && restoringActiveId !== activeId) select(active.id);
  }, [active.id, activeId, restoringActiveId, select]);

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

  const cycleZoom = useCallback(() => {
    const maxZoom = zoomForPixelPercent(fittedImageSize, sourceSize, MAX_PIXEL_ZOOM_PERCENT);
    const nextZoom = nextCycleZoom(zoom, fittedImageSize, sourceSize, maxZoom);
    if (nextZoom <= 1) {
      resetZoom();
      return;
    }
    // Center the zoom on the shooting focus area when one was parsed;
    // otherwise fall back to the image center.
    const anchor = focusRegionAnchor(mappedFocusRegions) ?? { x: 0.5, y: 0.5 };
    const { stage, image } = getSizes();
    setOffset(panFromNavigatorPoint(anchor, nextZoom, stage, image));
    setZoom(nextZoom);
  }, [fittedImageSize, getSizes, mappedFocusRegions, resetZoom, sourceSize, zoom]);

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

  const handleImageLoad = useCallback((size: DisplayedPreviewSize) => {
    setNaturalSize({ assetId: active.id, size });
  }, [active.id]);

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

  const stepSelection = useCallback((direction: -1 | 1) => {
    const next = assets[assets.indexOf(active) + direction];
    if (next) {
      select(next.id);
    } else if (direction > 0 && hasNextPage && !isFetchingNextPage) {
      fetchNextPage();
    }
  }, [active, assets, fetchNextPage, hasNextPage, isFetchingNextPage, select]);

  useMarkingShortcuts(assets, keyboardSuppressed, active);

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
      if (keyboardSuppressed) return;
      if (useWorkspaceStore.getState().settingsOpen || editableTarget(event.target)) return;
      if (matchesAction(event, shortcuts, "loupe.toggleFocusAreas")) {
        if (event.repeat) return;
        event.preventDefault();
        setFocusAreasVisible(!focusAreasVisible);
        return;
      }
      if (matchesAction(event, shortcuts, "loupe.cycleZoom")) {
        if (event.repeat) return;
        event.preventDefault();
        cycleZoom();
        return;
      }
      if (matchesAction(event, shortcuts, "loupe.previousAsset")) {
        event.preventDefault();
        stepSelection(-1);
        return;
      }
      if (matchesAction(event, shortcuts, "loupe.nextAsset")) {
        event.preventDefault();
        stepSelection(1);
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
  }, [cycleZoom, focusAreasVisible, keyboardSuppressed, setFocusAreasVisible, shortcuts, stepSelection]);

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

  const [navigation, setNavigation] = useState({ id: active.id, index: assets.indexOf(active), direction: 1 });
  const activeIndex = assets.indexOf(active);
  if (navigation.id !== active.id) {
    // Reset before committing the new renderer. An effect here would run
    // after its cache-hit layout effect and overwrite "complete" with loading.
    setZoom(1);
    setOffset({ x: 0, y: 0 });
    setRawPreviewStatus({ state: "loadingPreview" });
    setHeifStatus("probing");
    setNaturalSize(undefined);
    setHeifFullSize(undefined);
    setNavigation({ id: active.id, index: activeIndex, direction: activeIndex < navigation.index ? -1 : 1 });
  }
  const nearbyPreviewAssets = useMemo(() => (
    [0, navigation.direction, -navigation.direction, 2 * navigation.direction, -2 * navigation.direction]
      .flatMap((delta) => assets[activeIndex + delta] ?? [])
  ), [activeIndex, assets, navigation.direction]);
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
        {active.kind === "raw" && <RawDecoderPanel key={active.id} asset={active} compact
          pending={rawPreviewStatus.state === "loadingPreview" || rawPreviewStatus.state === "developingFull"}
          failed={rawPreviewStatus.state === "fullFailed"} t={t} />}
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
              : heifStatus === "failed"
                  ? t("fullQualityFailed")
                  : t("fullQualityLoading")}
          </div>
        ) : null}
        <div
          className="loupe__image"
          onContextMenu={(event) => onAssetContextMenu(event, active)}
          ref={imageRef}
          style={{
            width: fittedImageSize.width || undefined,
            height: fittedImageSize.height || undefined,
            transform: `translate3d(${offset.x}px, ${offset.y}px, 0)`,
          }}
        >
          <div
            className="loupe__render"
            data-asset-id={active.id}
            style={{
              width: fittedImageSize.width ? fittedImageSize.width * zoom : undefined,
              height: fittedImageSize.height ? fittedImageSize.height * zoom : undefined,
            }}
          >
            {displayedHeifArtifact !== heifPresentationIdentity ? <Thumbnail
              key={`thumbnail:${active.id}`}
              asset={active}
              large
              onImageLoad={handleImageLoad}
              onRawPreviewStatus={active.kind === "heif"
                ? handleHeifPreviewStatus
                : setRawPreviewStatus}
            /> : null}
            {heifUsesFullPresentation ? (
              <HeifTileCanvas
                key={`heif:${active.id}`}
                asset={active}
                displaySharpening={displaySharpening}
                onImageSize={handleHeifImageSize}
                previewDisplaySize={currentNaturalSize?.geometry?.displaySize}
                onArtifactDisplayed={handleHeifArtifactDisplayed}
                onStatus={setHeifStatus}
              />
            ) : null}
            {active ? (
              <FaceOverlay asset={active} enabled={faceBoxesVisible} t={t} />
            ) : null}
            {showFocusAreas && Boolean(currentNaturalSize || currentHeifSize) && mappedFocusRegions.length > 0 ? (
              <RegionOverlay hidden className="loupe__focus-overlay"
                descriptor={{ id: "focus.overlay", coordinateSpace: "displayNormalized", items: mappedFocusRegions.map((region, index) => ({ id: String(index), rect: { x: region.left, y: region.top, width: region.width, height: region.height }, state: region.syntheticFrame ? "estimated" : "known" })) }}
                regionClassName={(region) => `loupe__focus-frame ${region.state === "estimated" ? "is-estimated" : ""}`}
              />
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
          onPointerLeave={() => setHideControlsImmediately(true)}
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
            aria-keyshortcuts="f"
            aria-pressed={showFocusAreas}
            className={showFocusAreas ? "is-active" : ""}
            onClick={() => setFocusAreasVisible(!focusAreasVisible)}
            title={`${showFocusAreas ? t("hideFocusAreas") : t("showFocusAreas")} · ${t("focusShortcutHint")}`}
          >
            <Focus size={14} />
          </button>
          <button
            aria-pressed={faceBoxesVisible}
            className={faceBoxesVisible ? "is-active" : ""}
            onClick={() => setFaceBoxesVisible(!faceBoxesVisible)}
            title={`${faceBoxesVisible ? t("hideFaceBoxes") : t("showFaceBoxes")}`}
          >
            <ScanFace size={14} />
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
      <Filmstrip active={active} assets={assets} nearbyPreviewAssets={nearbyPreviewAssets} total={total}
        fetchNextPage={fetchNextPage} hasNextPage={hasNextPage} isFetchingNextPage={isFetchingNextPage}
        onAssetContextMenu={onAssetContextMenu} />
    </div>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}
