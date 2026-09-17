export const LAYOUT_SIZE_LIMITS = {
  leftPanel: { defaultValue: 224, min: 168, max: 420 },
  inspector: { defaultValue: 266, min: 220, max: 480 },
  filmstrip: { defaultValue: 116, min: 84, max: 300 },
} as const;

// Keep enough room for the browser controls and a useful thumbnail grid.
export const MIN_WORKSPACE_WIDTH = 360;

export type LayoutRegion = keyof typeof LAYOUT_SIZE_LIMITS;

export function clampLayoutSize(region: LayoutRegion, value: number): number {
  const limits = LAYOUT_SIZE_LIMITS[region];
  if (!Number.isFinite(value)) return limits.defaultValue;
  return Math.round(Math.min(limits.max, Math.max(limits.min, value)));
}

export function maxInspectorWidth(shellWidth: number, leftPanelWidth: number): number {
  const limits = LAYOUT_SIZE_LIMITS.inspector;
  if (!Number.isFinite(shellWidth) || shellWidth <= 0) return limits.max;
  const availableWidth = shellWidth - Math.max(0, leftPanelWidth) - MIN_WORKSPACE_WIDTH;
  return Math.max(limits.min, Math.min(limits.max, Math.floor(availableWidth)));
}
