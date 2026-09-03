export const GIB = 1024 ** 3;
export const MIN_CACHE_GB = 1;
export const MAX_CACHE_GB = 500;

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unitIndex;
  const digits = value >= 100 || unitIndex === 0 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[unitIndex]}`;
}

export function cacheLimitGb(bytes: number): number {
  return Math.round(bytes / GIB);
}

export function validCacheLimitGb(value: number): boolean {
  return Number.isInteger(value) && value >= MIN_CACHE_GB && value <= MAX_CACHE_GB;
}
