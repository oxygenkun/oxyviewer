import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// Spinner styles are a cross-file contract that nothing else checks.
//
// A spinner is a class name in a component and an animation rule in a
// stylesheet, and the two only meet if the selector can actually match. The
// face workbench shipped a `<Loader2 className="spin">` while the rule was
// scoped `.people-panel .spin` — a leftover from when the component was
// `PeoplePanel`. Nothing failed: the markup rendered, type-checked, and the
// tests passed, and the only symptom was a circle that did not turn.
//
// So this asserts the property that makes a spinner work regardless of where it
// is rendered: a spinner utility is a standalone rule (`.spin { ... }`), not one
// nested under an ancestor the component may not have.

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
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

/** Class names that look like a rotation spinner, as written in the markup. */
const spinnerClasses = (() => {
  const names = new Set<string>();
  const sources = readTree(frontendSource, ".ts").concat(readTree(frontendSource, ".tsx"));
  for (const [, text] of sources) {
    for (const match of text.matchAll(/className="([^"]*)"/g)) {
      for (const token of match[1].split(/\s+/)) {
        if (/spin/i.test(token)) names.add(token);
      }
    }
  }
  return [...names].sort();
})();

/** Every style rule in the frontend stylesheets, as selector and body. */
const cssRules = readTree(frontendSource, ".css").flatMap(([, text]) => {
  const withoutComments = text.replace(/\/\*[\s\S]*?\*\//g, "");
  return [...withoutComments.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
    selector: match[1].trim(),
    body: match[2],
  }));
});

describe("spinner classes are styled where they are rendered", () => {
  it("finds both sides of the contract", () => {
    // A parse that silently matched nothing would make the check below vacuous.
    expect(spinnerClasses.length).toBeGreaterThan(0);
    expect(cssRules.length).toBeGreaterThan(100);
  });

  it("defines a standalone animating rule for every spinner class in use", () => {
    const unstyled = spinnerClasses.filter(
      (name) =>
        !cssRules.some(
          (rule) =>
            rule.selector
              .split(",")
              .map((part) => part.trim())
              .includes(`.${name}`) && /animation:\s*(?!none)[^;}]+/.test(rule.body),
        ),
    );
    expect(
      unstyled,
      "these classes rotate nothing: give them a top-level `.name { animation: ... }` rule",
    ).toEqual([]);
  });
});
