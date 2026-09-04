export const LAYOUT_SIZE_LIMITS = {
  leftPanel: { defaultValue: 224, min: 168, max: 420 },
  inspector: { defaultValue: 266, min: 220, max: 480 },
  filmstrip: { defaultValue: 116, min: 84, max: 300 },
} as const;

export type LayoutRegion = keyof typeof LAYOUT_SIZE_LIMITS;

export function clampLayoutSize(region: LayoutRegion, value: number): number {
  const limits = LAYOUT_SIZE_LIMITS[region];
  if (!Number.isFinite(value)) return limits.defaultValue;
  return Math.round(Math.min(limits.max, Math.max(limits.min, value)));
}
