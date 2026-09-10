import { listen } from "@tauri-apps/api/event";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  cancelHeifDecode,
  heifTileUrl,
  isTauri,
  renewMediaResource,
  startHeifFull,
} from "../lib/api";
import { getBrowserImageDrawable, getBrowserImageResourceId, markBrowserImageReady } from "../lib/browserImageCache";
import { retainMediaResource, releaseUnretainedMediaResource } from "../lib/mediaResourceLease";
import { expectedHeifTiles, HeifTileProgressTracker } from "../lib/heifTileProgress";
import { perfMark, isPerfActive } from "../lib/perfProbe";
import { beginPreviewDebug } from "../lib/previewDebug";
import type {
  AssetSummary,
  HeifDecodeStatus,
  HeifDiagnostics,
  HeifStatusEvent,
  HeifTileReady,
  PreviewResult,
} from "../types";

let nextGeneration = 0;

interface HeifTileCanvasProps {
  asset: AssetSummary;
  displaySharpening: boolean;
  onArtifactDisplayed?: () => void;
  onImageSize: (size: { width: number; height: number }) => void;
  previewDisplaySize?: { width: number; height: number };
  onStatus: (status: HeifDecodeStatus, diagnostics?: HeifDiagnostics) => void;
}

export function HeifTileCanvas({
  asset,
  displaySharpening,
  onArtifactDisplayed,
  onImageSize,
  previewDisplaySize,
  onStatus,
}: HeifTileCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const previewSizeRef = useRef(previewDisplaySize);
  useLayoutEffect(() => { previewSizeRef.current = previewDisplaySize; }, [previewDisplaySize]);
  const [canvasVisible, setCanvasVisible] = useState(false);
  const memoryHostRef = useRef<HTMLDivElement>(null);
  const cacheIdentity = `${asset.id}:${asset.modifiedAtMs}:${displaySharpening}`;
  const memoryKey = `heif-display:${cacheIdentity}`;
  const restored = useRef(false);
  const [retainedPresentation, setRetainedPresentation] = useState<{ identity: string; resourceId?: string }>();
  useLayoutEffect(() => {
    restored.current = false;
    const image = getBrowserImageDrawable(memoryKey);
    const host = memoryHostRef.current;
    if (!image || !host) return;
    const width = image instanceof HTMLImageElement ? image.naturalWidth : image.width;
    const height = image instanceof HTMLImageElement ? image.naturalHeight : image.height;
    // Reattach the retained presentation itself. Drawing a 33 MP image into
    // a fresh canvas on every return costs hundreds of milliseconds in WebKit.
    image.className = "loupe__heif-canvas";
    host.appendChild(image);
    restored.current = true;
    const resourceId = getBrowserImageResourceId(memoryKey);
    setRetainedPresentation((current) => current?.identity === memoryKey && current.resourceId === resourceId
      ? current : { identity: memoryKey, resourceId });
    perfMark("image:loaded", {
      assetName: asset.name, large: true, stage: "full", renderLevel: "full", width, height,
    });
    perfMark("heif:memory-cache-hit", { assetName: asset.name, width, height });
    onImageSize({ width, height });
    onStatus("complete");
    onArtifactDisplayed?.();
    return () => { if (image.parentElement === host) image.remove(); };
  }, [asset.name, memoryKey, onImageSize, onStatus, onArtifactDisplayed]);
  const [cachedImage, setCachedImage] = useState<{
    identity: string;
    result: PreviewResult;
  }>();
  const [displayedCachedImage, setDisplayedCachedImage] = useState<typeof cachedImage>();
  const visibleCachedImage = displayedCachedImage?.identity === cacheIdentity
    ? displayedCachedImage.result : undefined;
  const candidateCachedImage = cachedImage?.identity === cacheIdentity ? cachedImage.result : undefined;
  const pendingCachedImage = candidateCachedImage?.url !== visibleCachedImage?.url
    ? candidateCachedImage : undefined;
  const [recovery, setRecovery] = useState(0);
  const resourceIdentity = [...new Set([
    ...[visibleCachedImage, pendingCachedImage].map((result) => result?.resource?.resourceId),
    retainedPresentation?.identity === memoryKey ? retainedPresentation.resourceId : undefined,
  ].filter(Boolean))].sort().join("\u0000");
  useLayoutEffect(() => {
    const ids = resourceIdentity ? resourceIdentity.split("\u0000") : [];
    if (!ids.length) return;
    const releases = ids.map(retainMediaResource);
    let disposed = false;
    let recovering = false;
    const renew = () => {
      if (recovering) return;
      void Promise.all(ids.map((id) => renewMediaResource(id).catch(() => false))).then((live) => {
        if (live.every(Boolean) || disposed || recovering) return;
        recovering = true;
        setRecovery((value) => value + 1);
      });
    };
    renew();
    const timer = window.setInterval(renew, 10_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      releases.forEach((release) => release());
    };
  }, [resourceIdentity]);

  useEffect(() => {
    if (!isTauri() || restored.current) return;
    setCanvasVisible(false);
    const generation = ++nextGeneration;
    const debug = __OXY_DEBUG__
      ? beginPreviewDebug({
          assetName: asset.name,
          stage: "heif-full-session",
          priority: "loupe",
        })
      : undefined;
    if (__OXY_DEBUG__) debug?.start();
    // Progress tracking also runs when the E2E performance harness is active,
    // so release builds still observe tile milestones (docs/PERF_E2E.md).
    const track = __OXY_DEBUG__ || isPerfActive();
    let sessionId: string | undefined;
    let disposed = false;
    const tileRequests = new AbortController();
    const unlisten: Array<() => void> = [];
    const stopListeners = () => {
      for (const stop of unlisten.splice(0)) stop();
    };
    const pendingTiles: HeifTileReady[] = [];
    const pendingStatuses: HeifStatusEvent[] = [];
    let progress: HeifTileProgressTracker | undefined;
    let backendComplete = false;
    let backendDiagnostics: HeifDiagnostics | undefined;
    let firstTileFetched = false;
    let firstPaintFrame: number | undefined;
    let completionPaintFrame: number | undefined;
    let frontendFetchWorkMs = 0;
    let frontendDrawWorkMs = 0;
    let slowestTileMs = 0;
    const tileQueue: HeifTileReady[] = [];
    let activeTileFetches = 0;
    const maxConcurrentTileFetches = 4;

    const detail = track
      ? () => ({
          ...(progress?.snapshot() ?? {}),
          frontendFetchWorkMs: Number(frontendFetchWorkMs.toFixed(1)),
          frontendDrawWorkMs: Number(frontendDrawWorkMs.toFixed(1)),
          slowestTileMs: Number(slowestTileMs.toFixed(1)),
          backendDiagnostics,
        })
      : undefined;

    const maybeComplete = () => {
      if (
        disposed
        || !backendComplete
        || !progress?.snapshot().allSettled
        || completionPaintFrame !== undefined
      ) return;
      completionPaintFrame = requestAnimationFrame(() => {
        completionPaintFrame = undefined;
        if (disposed) return;
        const completed = detail?.() ?? {};
        perfMark("heif:all-tiles-painted", { assetName: asset.name, ...completed });
        debug?.mark("all-tiles-painted", completed);
        if (progress?.snapshot().failed) {
          debug?.fail(new Error("one or more HEIF tiles failed to draw"), completed);
        } else {
          const canvas = canvasRef.current;
          if (canvas) {
            // A conflicting geometry cannot be progressively composited over
            // the old preview. Commit that complete canvas and its size together.
            setCanvasVisible(true);
            onImageSize({ width: canvas.width, height: canvas.height });
            markBrowserImageReady(memoryKey, { width: canvas.width, height: canvas.height }, canvas);
          }
          setDisplayedCachedImage(undefined);
          setCachedImage(undefined);
          debug?.complete(completed);
        }
      });
    };

    const drawTile = async (tile: HeifTileReady) => {
      const tileStarted = track ? performance.now() : 0;
      const fetchStarted = tileStarted;
      try {
        const response = await fetch(heifTileUrl(tile.url), { signal: tileRequests.signal });
        if (!response.ok) throw new Error(`tile fetch returned ${response.status}`);
        const encoded = tile.payload === "jpeg";
        const payload = encoded
          ? await response.blob()
          : new Uint8ClampedArray(await response.arrayBuffer());
        if (disposed) return;
        const fetchedAt = track ? performance.now() : 0;
        if (track) frontendFetchWorkMs += fetchedAt - fetchStarted;
        if (track && !firstTileFetched) {
          firstTileFetched = true;
          debug?.mark("first-tile-fetched", {
            x: tile.x,
            y: tile.y,
            bytes: payload instanceof Blob ? payload.size : payload.byteLength,
          });
        }
        const context = canvasRef.current?.getContext("2d");
        if (!context) throw new Error("HEIF canvas 2D context is unavailable");
        if (disposed) return;
        const drawStarted = track ? performance.now() : 0;
        if (payload instanceof Blob) {
          const bitmap = await createImageBitmap(payload);
          try {
            // The same canvas may now belong to a different selection. Decoding
            // is asynchronous even after the network response has completed.
            if (disposed) return;
            context.drawImage(bitmap, tile.x, tile.y, tile.width, tile.height);
          } finally {
            bitmap.close();
          }
        } else {
          context.putImageData(
            new ImageData(payload, tile.width, tile.height),
            tile.x,
            tile.y,
          );
        }
        const drawnAt = track ? performance.now() : 0;
        if (track) {
          frontendDrawWorkMs += drawnAt - drawStarted;
          slowestTileMs = Math.max(slowestTileMs, drawnAt - tileStarted);
        }
        const settled = progress?.settle(tile, true);
        if (settled?.firstDrawn) {
          const canvas = canvasRef.current;
          const preview = previewSizeRef.current;
          if (canvas && (!preview || (preview.width === canvas.width && preview.height === canvas.height))) {
            setCanvasVisible(true);
            onImageSize({ width: canvas.width, height: canvas.height });
          }
        }
        if (track && settled?.firstDrawn && firstPaintFrame === undefined) {
          firstPaintFrame = requestAnimationFrame(() => {
            firstPaintFrame = undefined;
            if (disposed) return;
            perfMark("heif:first-tile-painted", { assetName: asset.name, ...detail?.() });
            if (__OXY_DEBUG__) debug?.mark("first-tile-painted", detail?.());
          });
        }
        if (track && settled?.snapshot.allSettled) {
          debug?.mark("all-tiles-drawn", detail?.());
        }
      } catch (error) {
        if (!disposed) {
          const settled = progress?.settle(tile, false);
          perfMark("heif:tile-error", { assetName: asset.name, x: tile.x, y: tile.y });
          debug?.mark("tile-error", {
            x: tile.x,
            y: tile.y,
            error,
            progress: settled?.snapshot,
          });
        }
      } finally {
        activeTileFetches -= 1;
        pumpTileQueue();
        maybeComplete();
      }
    };

    const pumpTileQueue = () => {
      while (!disposed && activeTileFetches < maxConcurrentTileFetches) {
        const tile = tileQueue.shift();
        if (!tile) return;
        activeTileFetches += 1;
        void drawTile(tile);
      }
    };

    const handleTile = (tile: HeifTileReady) => {
      const received = progress?.receive(tile);
      if (received && !received.accepted) return;
      if (received?.first) {
        if (__OXY_DEBUG__) debug?.mark("first-tile-ready", {
          x: tile.x,
          y: tile.y,
          progress: received.snapshot,
        });
      }
      tileQueue.push(tile);
      pumpTileQueue();
    };

    const handleStatus = (event: HeifStatusEvent) => {
      onStatus(event.status, event.diagnostics);
      perfMark(`heif:backend-${event.status}`, {
        assetName: asset.name,
        diagnostics: event.diagnostics,
      });
      if (__OXY_DEBUG__) {
        debug?.mark(`backend-${event.status}`, {
          message: event.message,
          diagnostics: event.diagnostics,
        });
      }
      if (event.status === "complete") {
        backendComplete = true;
        backendDiagnostics = event.diagnostics;
        progress?.finishReceiving();
        maybeComplete();
      } else if (event.status === "failed") {
        if (__OXY_DEBUG__) debug?.fail(event.message ?? "HEIF backend decode failed", detail?.());
      } else if (event.status === "cancelled") {
        if (__OXY_DEBUG__) debug?.cancel(detail?.());
      }
    };

    const start = async () => {
      unlisten.push(await listen<HeifTileReady>("heif-tile-ready", ({ payload }) => {
        if (disposed || payload.generation !== generation) return;
        if (!sessionId) {
          pendingTiles.push(payload);
          return;
        }
        if (payload.sessionId === sessionId) handleTile(payload);
      }));
      if (disposed) {
        stopListeners();
        return;
      }
      unlisten.push(await listen<HeifStatusEvent>("heif-decode-status", ({ payload }) => {
        if (disposed || payload.generation !== generation) return;
        if (!sessionId) {
          pendingStatuses.push(payload);
          return;
        }
        if (payload.sessionId === sessionId) handleStatus(payload);
      }));
      if (disposed) {
        stopListeners();
        return;
      }
      if (__OXY_DEBUG__) debug?.mark("listeners-ready");
      const presentation = await startHeifFull(
        asset.path,
        generation,
        displaySharpening,
        tileRequests.signal,
      );
      if (presentation.delivery === "artifact") {
        const result = presentation.result;
        if (disposed) {
          if (result.resource) releaseUnretainedMediaResource(result.resource.resourceId);
          return;
        }
        setCachedImage({ identity: cacheIdentity, result });
        perfMark("heif:full-cache-hit", {
          assetName: asset.name,
          width: result.width,
          height: result.height,
        });
        // Geometry is committed with visible pixels in the image load handler.
        if (__OXY_DEBUG__) debug?.complete({ artifact: true });
        return;
      }
      const session = presentation.session;
      if (disposed) {
        await cancelHeifDecode(session.id);
        return;
      }
      sessionId = session.id;
      progress = new HeifTileProgressTracker(
        session.expectedTiles
          ?? expectedHeifTiles(session.width, session.height, session.tileSize),
      );
      const canvas = canvasRef.current;
      if (canvas) {
        canvas.width = session.width;
        canvas.height = session.height;
      }
      // A session describes pending pixels; it must not move the focus overlay.
      if (__OXY_DEBUG__) {
        debug?.mark("session-ready", {
          sessionId: session.id,
          width: session.width,
          height: session.height,
          tileSize: session.tileSize,
          expectedTiles: progress?.expected,
          backend: session.backend,
          acceleration: session.acceleration,
          status: session.status,
        });
      }
      onStatus(session.status);
      pendingTiles
        .filter((tile) => tile.sessionId === session.id)
        .forEach(handleTile);
      pendingStatuses
        .filter((event) => event.sessionId === session.id)
        .forEach(handleStatus);
    };
    // Defer one microtask so React StrictMode can dispose its throwaway effect,
    // but do not gate backend/session creation on a paint frame. A hidden or
    // temporarily occluded packaged WebView may suspend requestAnimationFrame;
    // full-detail HEIF work must still start independently of preview painting.
    queueMicrotask(() => {
      if (disposed) return;
      void start()
        .catch((error) => {
          stopListeners();
          if (disposed) return;
          onStatus("failed");
          if (__OXY_DEBUG__) debug?.fail(error, detail?.());
        });
    });

    return () => {
      disposed = true;
      tileRequests.abort();
      tileQueue.length = 0;
      if (firstPaintFrame !== undefined) cancelAnimationFrame(firstPaintFrame);
      if (completionPaintFrame !== undefined) cancelAnimationFrame(completionPaintFrame);
      stopListeners();
      if (sessionId) void cancelHeifDecode(sessionId);
      if (__OXY_DEBUG__) debug?.cancel(detail?.());
    };
  }, [
    recovery,
    asset.id,
    asset.modifiedAtMs,
    asset.path,
    cacheIdentity,
    memoryKey,
    displaySharpening,
    onImageSize,
    onStatus,
  ]);

  return <>
    <canvas key={memoryKey} className="loupe__heif-canvas" ref={canvasRef}
      style={{ visibility: canvasVisible ? "visible" : "hidden" }} />
    <div ref={memoryHostRef} style={{ display: "contents" }} />
    {visibleCachedImage ? <img
      key={visibleCachedImage.url}
      className="loupe__heif-canvas"
      src={visibleCachedImage.url}
      alt=""
      draggable={false}
    /> : null}
    {pendingCachedImage ? <img
      key={pendingCachedImage.url}
      className="loupe__heif-canvas"
      style={{ visibility: "hidden" }}
      src={pendingCachedImage.url}
      alt=""
      draggable={false}
      onError={() => { setCachedImage(undefined); onStatus("failed"); }}
      onLoad={(event) => {
        markBrowserImageReady(memoryKey, {
          width: event.currentTarget.naturalWidth, height: event.currentTarget.naturalHeight,
        }, event.currentTarget, pendingCachedImage.resource?.resourceId);
        setDisplayedCachedImage({ identity: cacheIdentity, result: pendingCachedImage });
        onImageSize({ width: pendingCachedImage.width, height: pendingCachedImage.height });
        onStatus("complete");
        onArtifactDisplayed?.();
        perfMark("image:loaded", {
          assetName: asset.name,
          large: true,
          stage: "full",
          renderLevel: "full",
          width: pendingCachedImage.width,
          height: pendingCachedImage.height,
        });
      }}
    /> : null}
  </>;
}
