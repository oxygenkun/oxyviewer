const TRAILING_SEPARATORS = /[\\/]+$/;
const LEADING_SEPARATORS = /^[\\/]+/;

function preferredSeparator(path: string): "/" | "\\" {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

function normalized(path: string): string {
  return path.replace(/\\/g, "/").replace(TRAILING_SEPARATORS, "");
}

function comparable(path: string): string {
  return /^[a-z]:/i.test(path) ? path.toLocaleLowerCase() : path;
}

export function relativeFolderPath(rootPath: string, path: string): string {
  const root = normalized(rootPath);
  const target = normalized(path);
  const comparableRoot = comparable(root);
  const comparableTarget = comparable(target);
  if (comparableTarget === comparableRoot) return ".";
  if (!comparableTarget.startsWith(`${comparableRoot}/`)) return path;

  return target
    .slice(root.length)
    .replace(LEADING_SEPARATORS, "")
    .replace(/[\\/]/g, preferredSeparator(rootPath));
}

export function parentFolderPath(path: string): string {
  const trimmed = path.replace(TRAILING_SEPARATORS, "");
  const separatorIndex = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (separatorIndex < 0) return path;
  if (separatorIndex === 0) return trimmed.slice(0, 1);
  if (/^[a-z]:$/i.test(trimmed.slice(0, separatorIndex))) return trimmed.slice(0, separatorIndex + 1);
  return trimmed.slice(0, separatorIndex);
}

export function isSameOrDescendantPath(ancestorPath: string, path: string): boolean {
  const ancestor = comparable(normalized(ancestorPath));
  const target = comparable(normalized(path));
  return target === ancestor || target.startsWith(`${ancestor}/`);
}

export type FileManagerKind = "finder" | "windowsExplorer" | "generic";

export function platformFileManager(userAgent = navigator.userAgent): FileManagerKind {
  if (/macintosh|mac os x/i.test(userAgent)) return "finder";
  if (/windows/i.test(userAgent)) return "windowsExplorer";
  return "generic";
}
