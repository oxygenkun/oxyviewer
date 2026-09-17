/**
 * Picks the image that may be decoded next in a progressive image chain.
 * Before the first stage has painted, later backend results must not skip it:
 * replacing an in-flight `<img>` makes the loupe flash back to its background.
 */
export function nextProgressiveStage<T>(
  hasDisplayedStage: boolean,
  stages: readonly (T | undefined)[],
): T | undefined {
  const available = stages.filter((stage): stage is T => stage !== undefined);
  return hasDisplayedStage ? available.at(-1) : available[0];
}
