import { expect, it } from 'vitest';
import css from '../../index.css?raw';

/** Reads the custom properties declared in the block that starts with `selector`. */
function readTokens(selector: string): Record<string, string> {
  // The formatter may switch the attribute selector to double quotes; compare quote-agnostic.
  const source = css.replaceAll('"', "'");
  const start = source.indexOf(`${selector} {`);
  if (start < 0) throw new Error(`Token block not found: ${selector}`);
  const body = source.slice(source.indexOf('{', start) + 1, source.indexOf('}', start));
  const tokens: Record<string, string> = {};
  for (const match of body.matchAll(/--([\w-]+):\s*([^;]+);/g)) {
    const [, name, value] = match;
    if (name && value) tokens[name] = value.trim();
  }
  return tokens;
}

function channel(value: number): number {
  const c = value / 255;
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function luminance(hex: string): number {
  const n = Number.parseInt(hex.slice(1), 16);
  return (
    0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255)
  );
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((hi ?? 0) + 0.05) / ((lo ?? 0) + 0.05);
}

/** [foreground token, background token, minimum ratio, why]. 4.5 for text, 3 for UI components. */
const pairs: ReadonlyArray<readonly [string, string, number, string]> = [
  ['fg', 'canvas', 4.5, 'body text on the window'],
  ['fg', 'surface', 4.5, 'body text on cards'],
  ['fg', 'surface-hover', 4.5, 'body text on a hovered card'],
  ['fg', 'surface-sunken', 4.5, 'body text on sunken surfaces'],
  ['muted', 'canvas', 4.5, 'secondary text on the window'],
  ['muted', 'surface', 4.5, 'secondary text on cards'],
  ['muted', 'surface-hover', 4.5, 'secondary text on a hovered card'],
  ['muted', 'surface-sunken', 4.5, 'secondary text in a neutral badge or segmented control'],
  ['accent-fg', 'accent', 4.5, 'label on the primary button'],
  ['accent-fg', 'accent-hover', 4.5, 'label on the hovered primary button'],
  ['accent-fg', 'accent-pressed', 4.5, 'label on the pressed primary button'],
  ['accent-text', 'accent-soft', 4.5, 'active navigation item and icon tiles'],
  ['accent-text', 'surface', 4.5, 'accent text on cards'],
  ['safe-fg', 'safe-bg', 4.5, 'Safe badge'],
  ['caution-fg', 'caution-bg', 4.5, 'Caution badge'],
  ['advanced-fg', 'advanced-bg', 4.5, 'Advanced badge'],
  ['toast-fg', 'toast-bg', 4.5, 'toast text'],
  ['toast-action', 'toast-bg', 4.5, 'toast action'],
  ['focus', 'canvas', 3, 'focus ring on the window'],
  ['focus', 'surface', 3, 'focus ring on cards'],
  ['line-strong', 'surface', 3, 'switch outline and input border'],
  ['line-strong', 'canvas', 3, 'control outline on the window'],
  ['accent', 'surface', 3, 'switch track when on'],
];

for (const [name, selector] of [
  ['light', ':root'],
  ['dark', ":root[data-theme='dark']"],
] as const) {
  it(`${name} theme: text and control colours meet WCAG AA`, () => {
    const tokens = readTokens(selector);
    for (const [fg, bg, minimum, why] of pairs) {
      const foreground = tokens[fg];
      const background = tokens[bg];
      if (!foreground?.startsWith('#') || !background?.startsWith('#')) {
        throw new Error(`Token pair ${fg}/${bg} must be hex colours in the ${name} theme`);
      }
      const ratio = contrast(foreground, background);
      expect(ratio, `${why}: ${fg} on ${bg} is ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(
        minimum,
      );
    }
  });
}

it('both themes define the same set of tokens', () => {
  const light = Object.keys(readTokens(':root')).sort();
  const dark = Object.keys(readTokens(":root[data-theme='dark']")).sort();
  expect(dark).toEqual(light);
});
