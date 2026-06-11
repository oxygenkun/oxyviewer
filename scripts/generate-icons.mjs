import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const output = resolve("apps/desktop/src-tauri/icons/icon.png");
const size = 1024;
const pixels = Buffer.alloc((size * 4 + 1) * size);

for (let y = 0; y < size; y += 1) {
  const row = y * (size * 4 + 1);
  pixels[row] = 0;
  for (let x = 0; x < size; x += 1) {
    const index = row + 1 + x * 4;
    const dx = x - size / 2;
    const dy = y - size / 2;
    const distance = Math.hypot(dx, dy);
    const angle = Math.atan2(dy, dx);
    const roundedCorner =
      Math.hypot(Math.max(Math.abs(dx) - 288, 0), Math.max(Math.abs(dy) - 288, 0));
    const inside = roundedCorner < 224;
    let red = 11;
    let green = 13;
    let blue = 15;
    let alpha = inside ? 255 : 0;

    if (inside) {
      const glow = Math.max(0, 1 - Math.hypot(dx + 180, dy + 220) / 700);
      red += Math.round(glow * 26);
      green += Math.round(glow * 30);
      blue += Math.round(glow * 31);
    }
    if (distance > 284 && distance < 316) {
      red = 57;
      green = 64;
      blue = 71;
    }
    if (distance < 280 && distance > 138) {
      const blade = Math.cos(angle * 4 + distance / 180);
      if (blade > 0.25) {
        red = 205 + Math.round(blade * 22);
        green = 235 + Math.round(blade * 18);
        blue = 104 + Math.round(blade * 18);
      }
    }
    if (distance < 140) {
      red = distance < 54 ? 228 : 17;
      green = distance < 54 ? 255 : 20;
      blue = distance < 54 ? 145 : 23;
    }
    if (distance > 126 && distance < 144) {
      red = 228;
      green = 255;
      blue = 145;
    }

    pixels[index] = red;
    pixels[index + 1] = green;
    pixels[index + 2] = blue;
    pixels[index + 3] = alpha;
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

