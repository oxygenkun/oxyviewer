import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const tag = process.argv[2];
if (!tag) {
  throw new Error("Usage: node scripts/extract-release-notes.mjs <version-or-tag>");
}

const version = tag.replace(/^v/, "");
const lines = readFileSync(resolve("CHANGELOG.md"), "utf8").split(/\r?\n/);
const regexCharacters = new Set("\\^$.*+?()[]{}|");
const escapedVersion = [...version].map((character) => regexCharacters.has(character) ? `\\${character}` : character).join("");
const heading = new RegExp(`^## \\[${escapedVersion}\\](?:\\s|$)`);
const start = lines.findIndex((line) => heading.test(line));
if (start < 0) {
  throw new Error(`CHANGELOG.md has no section for ${version}`);
}

const next = lines.findIndex(
  (line, index) =>
    index > start && (/^## \[/.test(line) || /^\[[^\]]+\]:\s/.test(line)),
);
const notes = lines.slice(start + 1, next < 0 ? lines.length : next).join("\n").trim();
if (!notes) {
  throw new Error(`CHANGELOG.md section for ${version} is empty`);
}

process.stdout.write(`${notes}\n`);
