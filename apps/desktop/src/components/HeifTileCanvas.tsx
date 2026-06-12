import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import {
  cancelHeifDecode,
  heifTileUrl,
  isTauri,
  startHeifDecode,
} from "../lib/api";
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
  onStatus: (status: HeifDecodeStatus, diagnostics?: HeifDiagnostics) => void;
}

export function HeifTileCanvas({
  asset,
  hardwareAcceleration,
  onStatus,
}: HeifTileCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (!isTauri()) return;
    const generation = ++nextGeneration;
    let sessionId: string | undefined;
    let disposed = false;
    const unlisten: Array<() => void> = [];
    const pendingTiles: HeifTileReady[] = [];

    const drawTile = async (tile: HeifTileReady) => {
        const response = await fetch(heifTileUrl(tile.url));
        if (!response.ok || disposed) return;
        const pixels = new Uint8ClampedArray(await response.arrayBuffer());
        const context = canvasRef.current?.getContext("2d");
        if (!context || disposed) return;
        context.putImageData(
          new ImageData(pixels, tile.width, tile.height),
          tile.x,
          tile.y,
        );
    };

    const start = async () => {
      unlisten.push(await listen<HeifTileReady>("heif-tile-ready", ({ payload }) => {
        if (disposed || payload.generation !== generation) return;
        if (!sessionId) {
          pendingTiles.push(payload);
          return;
        }
        if (payload.sessionId === sessionId) void drawTile(payload);
      }));
      unlisten.push(await listen<HeifStatusEvent>("heif-decode-status", ({ payload }) => {
        if (disposed || payload.generation !== generation || payload.sessionId !== sessionId) return;
        onStatus(payload.status, payload.diagnostics);
      }));
      const session = await startHeifDecode(asset.path, generation, hardwareAcceleration);
      if (disposed) {
        await cancelHeifDecode(session.id);
        return;
      }
      sessionId = session.id;
      const canvas = canvasRef.current;
      if (canvas) {
        canvas.width = session.width;
        canvas.height = session.height;
      }
      pendingTiles
        .filter((tile) => tile.sessionId === session.id)
        .forEach((tile) => void drawTile(tile));
      onStatus(session.status);
    };
    start().catch(() => onStatus("failed"));

    return () => {
      disposed = true;
      unlisten.forEach((stop) => stop());
      if (sessionId) void cancelHeifDecode(sessionId);
    };
  }, [asset.id, asset.path, hardwareAcceleration, onStatus]);

  return <canvas className="loupe__heif-canvas" ref={canvasRef} />;
}
