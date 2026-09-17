import type { HeifTileReady } from "@/types";

type TileIdentity = Pick<HeifTileReady, "x" | "y">;

export interface HeifTileProgressSnapshot {
  expected: number;
  received: number;
  settled: number;
  drawn: number;
  failed: number;
  allSettled: boolean;
}

function tileKey(tile: TileIdentity) {
  return `${tile.x}:${tile.y}`;
}

export function expectedHeifTiles(width: number, height: number, tileSize: number) {
  if (width <= 0 || height <= 0 || tileSize <= 0) return 0;
  return Math.ceil(width / tileSize) * Math.ceil(height / tileSize);
}

/** Keeps high-frequency HEIF tile accounting outside React state. */
export class HeifTileProgressTracker {
  private readonly received = new Set<string>();
  private readonly settled = new Set<string>();
  private drawn = 0;
  private failed = 0;

  constructor(public expected: number) {}

  finishReceiving() {
    this.expected = this.received.size;
    return this.snapshot();
  }

  receive(tile: TileIdentity) {
    const before = this.received.size;
    this.received.add(tileKey(tile));
    return {
      accepted: this.received.size !== before,
      first: before === 0 && this.received.size === 1,
      snapshot: this.snapshot(),
    };
  }

  settle(tile: TileIdentity, success: boolean) {
    const key = tileKey(tile);
    if (!this.received.has(key) || this.settled.has(key)) {
      return { accepted: false, firstDrawn: false, snapshot: this.snapshot() };
    }
    this.settled.add(key);
    if (success) this.drawn += 1;
    else this.failed += 1;
    return {
      accepted: true,
      firstDrawn: success && this.drawn === 1,
      snapshot: this.snapshot(),
    };
  }

  snapshot(): HeifTileProgressSnapshot {
    return {
      expected: this.expected,
      received: this.received.size,
      settled: this.settled.size,
      drawn: this.drawn,
      failed: this.failed,
      allSettled: this.expected > 0 && this.settled.size >= this.expected,
    };
  }
}
