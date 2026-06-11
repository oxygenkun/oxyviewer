import {
  Check,
  Eye,
  EyeOff,
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
  panFromNavigatorPoint,
  zoomAtPoint,
  type Point,
  type Size,
} from "../lib/loupe";
import { getAssetDetails } from "../lib/api";
import type { MessageKey } from "../lib/i18n";
import { useWorkspaceStore } from "../store";
import type { AssetSummary, NavigatorPosition } from "../types";
import { Thumbnail } from "./Thumbnail";

interface LoupeProps {
  assets: AssetSummary[];
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

export function Loupe({ assets, t }: LoupeProps) {
  const activeId = useWorkspaceStore((state) => state.activeId);
  const select = useWorkspaceStore((state) => state.select);
  const navigatorVisible = useWorkspaceStore((state) => state.navigatorVisible);
  const navigatorPosition = useWorkspaceStore((state) => state.navigatorPosition);
  const setNavigatorVisible = useWorkspaceStore((state) => state.setNavigatorVisible);
  const setNavigatorPosition = useWorkspaceStore((state) => state.setNavigatorPosition);
  const active = assets.find((asset) => asset.id === activeId) ?? assets[0];
  const stageRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLDivElement>(null);
  const navigatorRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; start: Point; offset: Point } | undefined>(undefined);
  const navigatorDragRef = useRef<number | undefined>(undefined);
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState<Point>({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [layoutVersion, setLayoutVersion] = useState(0);
  const [stageContentSize, setStageContentSize] = useState<Size>({ width: 0, height: 0 });
  const [naturalSize, setNaturalSize] = useState<{ assetId: string; size: Size } | undefined>(undefined);
  const details = useQuery({
    queryKey: ["asset-details", active.id],
    queryFn: () => getAssetDetails(active),
  });

  const sourceSize = naturalSize?.assetId === active.id
    ? naturalSize.size
    : details.data?.width && details.data.height
      ? { width: details.data.width, height: details.data.height }
      : DEFAULT_IMAGE_SIZE;
  const fittedImageSize = fitSize(stageContentSize, sourceSize);
  const navigatorImageSize = fitSize(NAVIGATOR_MAX_SIZE, sourceSize);

  const getSizes = useCallback(() => ({
    stage: elementSize(stageRef.current),
    image: elementSize(imageRef.current),
  }), []);

  const setZoomAroundPoint = useCallback((next: number, pointFromStageCenter: Point) => {
    const nextZoom = clampZoom(next);
    const { stage, image } = getSizes();
    setOffset((current) => zoomAtPoint(zoom, nextZoom, current, pointFromStageCenter, stage, image));
    setZoom(nextZoom);
  }, [getSizes, zoom]);

  const resetZoom = useCallback(() => {
    setZoom(1);
    setOffset({ x: 0, y: 0 });
  }, []);

  useEffect(() => {
    resetZoom();
  }, [active.id, resetZoom]);

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

  const panFromNavigatorEvent = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    const rect = navigatorRef.current?.getBoundingClientRect();
    if (!rect) return;
    const { stage, image } = getSizes();
    setOffset(panFromNavigatorPoint({
      x: Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)),
      y: Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height)),
    }, zoom, stage, image));
  }, [getSizes, zoom]);

  const navigatorViewport = useMemo(() => {
    const { stage, image } = getSizes();
    if (!stage.width || !image.width) return { left: 0, top: 0, width: 1, height: 1 };
    return getNavigatorViewport(zoom, offset, stage, image);
  }, [getSizes, layoutVersion, offset, zoom]);

  const zoomLabel = `${Math.round(zoom * 100)}%`;
  const positionLabels: Record<NavigatorPosition, MessageKey> = {
    "top-left": "topLeft",
    "top-right": "topRight",
    "bottom-left": "bottomLeft",
    "bottom-right": "bottomRight",
  };

  return (
    <div className="loupe">
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
        </div>
        <div
          className="loupe__image"
          ref={imageRef}
          style={{
            width: fittedImageSize.width || undefined,
            height: fittedImageSize.height || undefined,
            transform: `translate3d(${offset.x}px, ${offset.y}px, 0) scale(${zoom})`,
          }}
        >
          <Thumbnail
            asset={active}
            large
            onImageLoad={(size) => setNaturalSize({ assetId: active.id, size })}
          />
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
                event.currentTarget.setPointerCapture(event.pointerId);
                navigatorDragRef.current = event.pointerId;
                panFromNavigatorEvent(event);
              }}
              onPointerMove={(event) => {
                if (navigatorDragRef.current === event.pointerId) panFromNavigatorEvent(event);
              }}
              onPointerUp={(event) => {
                if (navigatorDragRef.current === event.pointerId) navigatorDragRef.current = undefined;
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
        <div className="loupe__controls">
          <button onClick={() => setZoomAroundPoint(zoom / 1.25, { x: 0, y: 0 })} title={t("zoomOut")}>
            <Minus size={14} />
          </button>
          <button className="loupe__zoom-label" onClick={resetZoom} title={t("resetZoom")}>
            {zoomLabel}
          </button>
          <button onClick={() => setZoomAroundPoint(zoom * 1.25, { x: 0, y: 0 })} title={t("zoomIn")}>
            <Plus size={14} />
          </button>
          <i />
          <button
            className={navigatorVisible ? "is-active" : ""}
            onClick={() => setNavigatorVisible(!navigatorVisible)}
            title={navigatorVisible ? t("hideNavigator") : t("showNavigator")}
          >
            {navigatorVisible ? <Eye size={14} /> : <EyeOff size={14} />}
          </button>
          <button
            className={settingsOpen ? "is-active" : ""}
            onClick={() => setSettingsOpen((open) => !open)}
            title={t("navigatorSettings")}
          >
            <Settings2 size={14} />
          </button>
        </div>
        {settingsOpen ? (
          <div className="loupe__settings">
            <header>
              <span>{t("navigatorSettings")}</span>
              <small>{t("wheelZoomHint")}</small>
            </header>
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
      <div
        className="filmstrip"
        onWheel={(event) => {
          if (event.deltaY === 0) return;
          event.preventDefault();
          event.currentTarget.scrollLeft += event.deltaX + event.deltaY;
        }}
      >
        {assets.slice(0, 40).map((asset) => (
          <button
            key={asset.id}
            className={active.id === asset.id ? "is-active" : ""}
            onClick={() => select(asset.id)}
            title={asset.name}
          >
            <Thumbnail asset={asset} />
          </button>
        ))}
      </div>
    </div>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} KB`;
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}
