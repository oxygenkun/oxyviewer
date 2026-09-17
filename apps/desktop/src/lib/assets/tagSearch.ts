export function splitTagInput(text: string): { filename: string; needle?: string } {
  const match = /(?:^|\s)#([^#]*)$/.exec(text);
  return match ? { filename: text.slice(0, match.index).trimEnd(), needle: match[1] }
    : { filename: text };
}
