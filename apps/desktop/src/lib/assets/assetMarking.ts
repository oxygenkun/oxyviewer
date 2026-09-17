import type { MetadataPatch } from "@/types";
import type { ShortcutAction } from "@/lib/ui/shortcuts";

export const MARKING_COLOR_ACTIONS = [
  ["marking.colorRed", "Red"],
  ["marking.colorYellow", "Yellow"],
  ["marking.colorGreen", "Green"],
  ["marking.colorBlue", "Blue"],
] as const;

export const MARKING_ACTIONS: ShortcutAction[] = [
  "marking.rating1",
  "marking.rating2",
  "marking.rating3",
  "marking.rating4",
  "marking.rating5",
  "marking.clearRating",
  ...MARKING_COLOR_ACTIONS.map(([action]) => action),
];

export function ratingForAction(action: ShortcutAction): number | null | undefined {
  if (action === "marking.clearRating") return null;
  const match = /^marking\.rating([1-5])$/.exec(action);
  return match ? Number(match[1]) : undefined;
}

export function colorLabelForAction(action: ShortcutAction): string | undefined {
  return MARKING_COLOR_ACTIONS.find((entry) => entry[0] === action)?.[1];
}

export function markingPatchForAction(
  action: ShortcutAction,
  currentColorLabel: string | undefined,
): MetadataPatch | undefined {
  const rating = ratingForAction(action);
  if (rating !== undefined) return { rating };
  const label = colorLabelForAction(action);
  if (!label) return undefined;
  // Color keys toggle so a color can also be removed from the keyboard.
  return { colorLabel: currentColorLabel?.toLowerCase() === label.toLowerCase() ? null : label };
}
