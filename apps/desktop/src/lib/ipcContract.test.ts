import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Cross-language contract check.
//
// Nothing else in the suite crosses the Rust/TypeScript boundary: a command
// renamed on one side, or an event emitted under a different name than the
// frontend listens for, still type-checks, still compiles, and fails only when
// a user clicks. This reads both sources and asserts they agree.
//
// It parses source text rather than executing either side, so it catches the
// mistakes that are otherwise invisible: a `#[tauri::command]` that was never
// registered, a registered name with no function, an `invoke` for a command
// that does not exist, and a `listen` for an event nothing emits.

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
const tauriSource = join(repositoryRoot, "apps/desktop/src-tauri/src");
const frontendSource = join(repositoryRoot, "apps/desktop/src");

function readTree(directory: string, extension: string): Array<[string, string]> {
  const files: Array<[string, string]> = [];
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) {
      files.push(...readTree(path, extension));
    } else if (entry.endsWith(extension) && !entry.includes(".test.")) {
      files.push([path, readFileSync(path, "utf8")]);
    }
  }
  return files;
}

function matches(source: string, pattern: RegExp, group = 1): string[] {
  return [...source.matchAll(pattern)].map((match) => match[group]);
}

const rustSources = readTree(tauriSource, ".rs");
const rustText = rustSources.map(([, text]) => text).join("\n");

/** Commands registered in the single `generate_handler!` list. */
const registeredCommands = (() => {
  const list = /generate_handler!\[([\s\S]*?)\]/.exec(rustText)?.[1] ?? "";
  return list
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
})();

/** Commands actually declared with the attribute. */
const declaredCommands = matches(
  rustText,
  /#\[tauri::command\]\s*(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+(\w+)/g,
);

/** Commands the frontend calls, including from components. */
const frontendText = readTree(frontendSource, ".ts")
  .concat(readTree(frontendSource, ".tsx"))
  .map(([, text]) => text)
  .join("\n");
const invokedCommands = [
  ...matches(frontendText, /invoke<[^>]*>\(\s*"([^"]+)"/g),
  ...matches(frontendText, /invoke\(\s*"([^"]+)"/g),
];

/** Events the frontend subscribes to. */
const listenedEvents = [
  ...matches(frontendText, /listen<[^>]*>\(\s*"([^"]+)"/g),
  ...matches(frontendText, /listen\(\s*"([^"]+)"/g),
];

/** Events Rust emits, whether inline or through a named constant. */
const emittedEvents = [
  ...matches(rustText, /emit\(\s*"([^"]+)"/g),
  ...matches(rustText, /_EVENT:\s*&str\s*=\s*"([^"]+)"/g),
];

describe("Rust and TypeScript agree on the IPC surface", () => {
  it("extracts both sides of the contract", () => {
    // A parse that silently matched nothing would make every check below pass.
    expect(declaredCommands.length).toBeGreaterThan(40);
    expect(registeredCommands.length).toBeGreaterThan(40);
    expect(invokedCommands.length).toBeGreaterThan(20);
    expect(listenedEvents.length).toBeGreaterThan(5);
    expect(emittedEvents.length).toBeGreaterThan(5);
  });

  it("registers every declared command", () => {
    const missing = declaredCommands.filter((name) => !registeredCommands.includes(name));
    expect(missing, "declare these in generate_handler!").toEqual([]);
  });

  it("registers nothing that is not a command", () => {
    const unknown = registeredCommands.filter((name) => !declaredCommands.includes(name));
    expect(unknown, "these registered names have no #[tauri::command]").toEqual([]);
  });

  it("only calls commands that exist and are registered", () => {
    const unknown = [...new Set(invokedCommands)].filter(
      (name) => !registeredCommands.includes(name),
    );
    expect(unknown, "the frontend invokes commands Rust does not expose").toEqual([]);
  });

  it("only listens for events something emits", () => {
    const unknown = [...new Set(listenedEvents)].filter(
      (name) => !emittedEvents.includes(name),
    );
    expect(unknown, "the frontend listens for events nothing emits").toEqual([]);
  });

  it("does not invoke a command with a computed name", () => {
    // A template literal would evade the checks above, so it must not appear.
    expect(frontendText).not.toMatch(/invoke<[^>]*>\(\s*`/);
    expect(frontendText).not.toMatch(/invoke\(\s*`/);
  });
});
