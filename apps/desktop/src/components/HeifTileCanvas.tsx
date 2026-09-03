import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import {
  cancelHeifDecode,
  getCachedHeifFull,
  heifTileUrl,
  isTauri,
  startHeifDecode,
} from "../lib/api";
import { expectedHeifTiles, HeifTileProgressTracker } from "../lib/heifTileProgress";
import { perfMark, isPerfActive } from "../lib/perfProbe";
import { beginPreviewDebug } from "../lib/previewDebug";
import type {
  AssetSummary,
  HeifDecodeSession,
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
  hardwareAcceleration: boolean;
  onImageSize: (size: { width: number; height: number }) => void;
  onStatus: (status: HeifDecodeStatus, diagnostics?: HeifDiagnostics) => void;
}

export function HeifTileCanvas({
  asset,
  displaySharpening,
  hardwareAcceleration,
  onImageSize,
  onStatus,
}: HeifTileCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const cacheIdentity = `${asset.id}:${asset.modifiedAtMs}:${hardwareAcceleration}:${displaySharpening}`;
  const [cachedImage, setCachedImage] = useState<{
    identity: string;
    result: PreviewResult;
  }>();
  const visibleCachedImage = cachedImage?.identity === cacheIdentity
    ? cachedImage.result
    : undefined;

  useEffect(() => {
    if (!isTauri()) return;
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
    const unlisten: Array<() => void> = [];
    const pendingTiles: HeifTileReady[] = [];
    const pendingStatuses: HeifStatusEvent[] = [];
    let progress: HeifTileProgressTracker | undefined;
    let backendComplete = false;
    let backendDiagnostics: HeifDiagnostics | undefined;
    let firstTileFetched = false;
    let firstPaintFrame: number | undefined;
    let completionPaintFrame: number | undefined;
    let startFrame: number | undefined;
    let frontendFetchWorkMs = 0;
    let frontendDrawWorkMs = 0;
    let slowestTileMs = 0;
    const tileQueue: HeifTileReady[] = [];
    let activeTileFetches = 0;
    const maxConcurrentTileFetches = 4;

    const loadCachedImage = async () => {
      try {
        const result = await getCachedHeifFull(asset.path);
        if (disposed || !result) return false;
        setCachedImage({ identity: cacheIdentity, result });
        perfMark("heif:full-cache-hit", {
          assetName: asset.name,
          width: result.width,
          height: result.height,
        });
        onImageSize({ width: result.width, height: result.height });
        onStatus("complete");
        return true;
      } catch (error) {
        if (__OXY_DEBUG__) debug?.mark("cache-lookup-failed", { error });
        return false;
      }
    };

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
        !track
        || disposed
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
          debug?.complete(completed);
        }
      });
    };

    const drawTile = async (tile: HeifTileReady) => {
      const tileStarted = track ? performance.now() : 0;
      const fetchStarted = tileStarted;
      try {
        const response = await fetch(heifTileUrl(tile.url));
        if (!response.ok) throw new Error(`tile fetch returned ${response.status}`);
        const encoded = response.headers.get("content-type") === "image/jpeg";
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
          context.drawImage(bitmap, tile.x, tile.y, tile.width, tile.height);
          bitmap.close();
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
        const settled = track ? progress?.settle(tile, true) : undefined;
        if (settled?.firstDrawn && firstPaintFrame === undefined) {
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
        if (track) {
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
      const received = track ? progress?.receive(tile) : undefined;
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
        if (track) {
          backendComplete = true;
          backendDiagnostics = event.diagnostics;
          progress?.finishReceiving();
          maybeComplete();
        }
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
      unlisten.push(await listen<HeifStatusEvent>("heif-decode-status", ({ payload }) => {
        if (disposed || payload.generation !== generation) return;
        if (!sessionId) {
          pendingStatuses.push(payload);
          return;
        }
        if (payload.sessionId === sessionId) handleStatus(payload);
      }));
      unlisten.push(await listen<HeifDecodeSession>("heif-cache-ready", ({ payload }) => {
        if (
          disposed
          || payload.generation !== generation
          || payload.id !== sessionId
        ) return;
        perfMark("heif:cache-written", { assetName: asset.name });
      }));
      if (__OXY_DEBUG__) debug?.mark("listeners-ready");
      const session = await startHeifDecode(
        asset.path,
        generation,
        hardwareAcceleration,
        displaySharpening,
      );
      if (disposed) {
        await cancelHeifDecode(session.id);
        return;
      }
      sessionId = session.id;
      progress = track
        ? new HeifTileProgressTracker(
            session.expectedTiles
              ?? expectedHeifTiles(session.width, session.height, session.tileSize),
          )
        : undefined;
      const canvas = canvasRef.current;
      if (canvas) {
        canvas.width = session.width;
        canvas.height = session.height;
      }
      onImageSize({ width: session.width, height: session.height });
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
    // Defer backend creation by one frame. React StrictMode intentionally
    // mounts, cleans up, and remounts effects; starting immediately lets the
    // throwaway mount create an orphan decode before it has a session id to
    // cancel. The cleanup below cancels that scheduled start, so only the
    // durable mount reaches the backend.
    startFrame = requestAnimationFrame(() => {
      startFrame = undefined;
      void loadCachedImage()
        .then((cacheHit) => {
          if (cacheHit || disposed) {
            if (cacheHit && __OXY_DEBUG__) debug?.complete({ cacheHit: true });
            return;
          }
          return start();
        })
        .catch((error) => {
          onStatus("failed");
          if (__OXY_DEBUG__) debug?.fail(error, detail?.());
        });
    });

    return () => {
      disposed = true;
      if (startFrame !== undefined) cancelAnimationFrame(startFrame);
      if (firstPaintFrame !== undefined) cancelAnimationFrame(firstPaintFrame);
      if (completionPaintFrame !== undefined) cancelAnimationFrame(completionPaintFrame);
      unlisten.forEach((stop) => stop());
      if (sessionId) void cancelHeifDecode(sessionId);
      if (__OXY_DEBUG__) debug?.cancel(detail?.());
    };
  }, [
    asset.id,
    asset.modifiedAtMs,
    asset.path,
    cacheIdentity,
    displaySharpening,
    hardwareAcceleration,
    onImageSize,
    onStatus,
  ]);

  return visibleCachedImage ? (
    <img
      className="loupe__heif-canvas"
      src={visibleCachedImage.url}
      alt=""
      draggable={false}
    />
  ) : (
    <canvas className="loupe__heif-canvas" ref={canvasRef} />
  );
}
