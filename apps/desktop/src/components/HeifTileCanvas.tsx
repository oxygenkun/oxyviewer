import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import {
  cancelHeifDecode,
  heifTileUrl,
  isTauri,
  startHeifDecode,
} from "../lib/api";
import { expectedHeifTiles, HeifTileProgressTracker } from "../lib/heifTileProgress";
import { beginPreviewDebug } from "../lib/previewDebug";
import type {
  AssetSummary,
  HeifDecodeStatus,
  HeifDiagnostics,
  HeifStatusEvent,
  HeifTileReady,
} from "../types";

let nextGeneration = 0;

interface HeifTileCanvasProps {
  asset: AssetSummary;
  hardwareAcceleration: boolean;
  onImageSize: (size: { width: number; height: number }) => void;
  onStatus: (status: HeifDecodeStatus, diagnostics?: HeifDiagnostics) => void;
}

export function HeifTileCanvas({
  asset,
  hardwareAcceleration,
  onImageSize,
  onStatus,
}: HeifTileCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

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
    let frontendFetchWorkMs = 0;
    let frontendDrawWorkMs = 0;
    let slowestTileMs = 0;

    const detail = __OXY_DEBUG__
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
        !__OXY_DEBUG__
        || disposed
        || !debug
        || !backendComplete
        || !progress?.snapshot().allSettled
        || completionPaintFrame !== undefined
      ) return;
      completionPaintFrame = requestAnimationFrame(() => {
        completionPaintFrame = undefined;
        if (disposed) return;
        const completed = detail?.() ?? {};
        debug.mark("all-tiles-painted", completed);
        if (progress?.snapshot().failed) {
          debug.fail(new Error("one or more HEIF tiles failed to draw"), completed);
        } else {
          debug.complete(completed);
        }
      });
    };

    const drawTile = async (tile: HeifTileReady) => {
      const tileStarted = __OXY_DEBUG__ ? performance.now() : 0;
      const fetchStarted = tileStarted;
      try {
        const response = await fetch(heifTileUrl(tile.url));
        if (!response.ok) throw new Error(`tile fetch returned ${response.status}`);
        if (disposed) return;
        const pixels = new Uint8ClampedArray(await response.arrayBuffer());
        const fetchedAt = __OXY_DEBUG__ ? performance.now() : 0;
        if (__OXY_DEBUG__) frontendFetchWorkMs += fetchedAt - fetchStarted;
        if (__OXY_DEBUG__ && !firstTileFetched) {
          firstTileFetched = true;
          debug?.mark("first-tile-fetched", {
            x: tile.x,
            y: tile.y,
            bytes: pixels.byteLength,
          });
        }
        const context = canvasRef.current?.getContext("2d");
        if (!context) throw new Error("HEIF canvas 2D context is unavailable");
        if (disposed) return;
        const drawStarted = __OXY_DEBUG__ ? performance.now() : 0;
        context.putImageData(
          new ImageData(pixels, tile.width, tile.height),
          tile.x,
          tile.y,
        );
        const drawnAt = __OXY_DEBUG__ ? performance.now() : 0;
        if (__OXY_DEBUG__) {
          frontendDrawWorkMs += drawnAt - drawStarted;
          slowestTileMs = Math.max(slowestTileMs, drawnAt - tileStarted);
        }
        const settled = __OXY_DEBUG__ ? progress?.settle(tile, true) : undefined;
        if (settled?.firstDrawn && firstPaintFrame === undefined) {
          firstPaintFrame = requestAnimationFrame(() => {
            firstPaintFrame = undefined;
            if (!disposed && __OXY_DEBUG__) debug?.mark("first-tile-painted", detail?.());
          });
        }
        if (__OXY_DEBUG__ && settled?.snapshot.allSettled) {
          debug?.mark("all-tiles-drawn", detail?.());
        }
      } catch (error) {
        if (__OXY_DEBUG__) {
          const settled = progress?.settle(tile, false);
          debug?.mark("tile-error", {
            x: tile.x,
            y: tile.y,
            error,
            progress: settled?.snapshot,
          });
        }
      } finally {
        maybeComplete();
      }
    };

    const handleTile = (tile: HeifTileReady) => {
      const received = __OXY_DEBUG__ ? progress?.receive(tile) : undefined;
      if (received && !received.accepted) return;
      if (received?.first) {
        if (__OXY_DEBUG__) debug?.mark("first-tile-ready", {
          x: tile.x,
          y: tile.y,
          progress: received.snapshot,
        });
      }
      void drawTile(tile);
    };

    const handleStatus = (event: HeifStatusEvent) => {
      onStatus(event.status, event.diagnostics);
      if (__OXY_DEBUG__) {
        debug?.mark(`backend-${event.status}`, {
          message: event.message,
          diagnostics: event.diagnostics,
        });
      }
      if (event.status === "complete") {
        if (__OXY_DEBUG__) {
          backendComplete = true;
          backendDiagnostics = event.diagnostics;
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
      if (__OXY_DEBUG__) debug?.mark("listeners-ready");
      const session = await startHeifDecode(asset.path, generation, hardwareAcceleration);
      if (disposed) {
        await cancelHeifDecode(session.id);
        return;
      }
      sessionId = session.id;
      progress = __OXY_DEBUG__ && debug
        ? new HeifTileProgressTracker(
            expectedHeifTiles(session.width, session.height, session.tileSize),
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
    start().catch((error) => {
      onStatus("failed");
      if (__OXY_DEBUG__) debug?.fail(error, detail?.());
    });

    return () => {
      disposed = true;
      if (firstPaintFrame !== undefined) cancelAnimationFrame(firstPaintFrame);
      if (completionPaintFrame !== undefined) cancelAnimationFrame(completionPaintFrame);
      unlisten.forEach((stop) => stop());
      if (sessionId) void cancelHeifDecode(sessionId);
      if (__OXY_DEBUG__) debug?.cancel(detail?.());
    };
  }, [asset.id, asset.path, hardwareAcceleration, onImageSize, onStatus]);

  return <canvas className="loupe__heif-canvas" ref={canvasRef} />;
}
