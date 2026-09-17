import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Resolved from this file so the generator works from any working directory.
const APP_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = resolve(APP_ROOT, "src-tauri/icons/icon.png");
const size = 1024;
const pixels = Buffer.alloc((size * 4 + 1) * size);
const center = size / 2;
const markColor = [210, 255, 72];
const strokeWidth = 34;
const outerRadius = 284;

const segments = [
  [[578, 398], [741, 681]],
  [[446, 398], [772, 398]],
  [[381, 512], [544, 230]],
  [[446, 626], [283, 343]],
  [[578, 626], [252, 626]],
  [[643, 512], [480, 794]],
];

function distanceToSegment(x, y, [ax, ay], [bx, by]) {
  const abx = bx - ax;
  const aby = by - ay;
  const projection = Math.max(
    0,
    Math.min(1, ((x - ax) * abx + (y - ay) * aby) / (abx * abx + aby * aby)),
  );
  return Math.hypot(x - (ax + projection * abx), y - (ay + projection * aby));
}

function coverage(distance, halfWidth) {
  return Math.max(0, Math.min(1, halfWidth + 0.75 - distance));
}

for (let y = 0; y < size; y += 1) {
  const row = y * (size * 4 + 1);
  pixels[row] = 0;
  for (let x = 0; x < size; x += 1) {
    const index = row + 1 + x * 4;
    const dx = x - center;
    const dy = y - center;
    const roundedCorner =
      Math.hypot(Math.max(Math.abs(dx) - 288, 0), Math.max(Math.abs(dy) - 288, 0));
    const inside = roundedCorner < 224;
    const radial = Math.hypot(dx, dy);
    const backgroundLift = Math.max(0, 1 - Math.hypot(dx + 150, dy + 200) / 760);
    const base = [
      9 + Math.round(backgroundLift * 14),
      12 + Math.round(backgroundLift * 17),
      13 + Math.round(backgroundLift * 18),
    ];

    let lineDistance = Math.abs(radial - outerRadius);
    for (const [start, end] of segments) {
      lineDistance = Math.min(lineDistance, distanceToSegment(x, y, start, end));
    }
    const markCoverage = coverage(lineDistance, strokeWidth / 2);

    pixels[index] = Math.round(base[0] * (1 - markCoverage) + markColor[0] * markCoverage);
    pixels[index + 1] = Math.round(base[1] * (1 - markCoverage) + markColor[1] * markCoverage);
    pixels[index + 2] = Math.round(base[2] * (1 - markCoverage) + markColor[2] * markCoverage);
    pixels[index + 3] = inside ? 255 : 0;
  }
}

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const name = Buffer.from(type);
  const output = Buffer.alloc(12 + data.length);
  output.writeUInt32BE(data.length, 0);
  name.copy(output, 4);
  data.copy(output, 8);
  output.writeUInt32BE(crc32(Buffer.concat([name, data])), 8 + data.length);
  return output;
}

const header = Buffer.alloc(13);
header.writeUInt32BE(size, 0);
header.writeUInt32BE(size, 4);
header[8] = 8;
header[9] = 6;
const png = Buffer.concat([
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
  chunk("IHDR", header),
  chunk("IDAT", deflateSync(pixels, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, png);
console.log(`Generated ${output}`);
