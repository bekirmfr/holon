// Colour has one meaning each (web-console-L4.md §3, principle 5): in every look (Clean dark,
// Clean light, Terminal), the statement graph's kinds each have a colour of their own, and only
// a call outside is teal. Read from styles.css itself: its tokens per look, and each kind's `--k`.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(process.cwd(), "src", "styles.css"), "utf8");

/** The custom properties a block declares, from the first `{` after `head` to its `}`. */
function tokensOf(head: string): Record<string, string> {
  const at = css.indexOf(`${head} {`);
  if (at < 0) throw new Error(`no ${head} in styles.css`);
  const body = css.slice(css.indexOf("{", at) + 1, css.indexOf("}", at));
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}

const dark = tokensOf("\n:root");
const looks: Record<string, Record<string, string>> = {
  "Clean dark": dark,
  "Clean light": { ...dark, ...tokensOf(':root[data-theme="light"]') },
  Terminal: { ...dark, ...tokensOf(':root[data-ui="b"]') },
};

/** Each kind's colour token: the Clean rule, else Terminal's own where it has one. */
function kindTokens(look: string): Record<string, string> {
  const rule = (prefix: string) => Object.fromEntries([...css.matchAll(new RegExp(`^${prefix}\\.hl-sg--(\\w+) \\{ --k: var\\((--[\\w-]+)\\); \\}`, "gm"))].map((m) => [m[1], m[2]]));
  return { ...rule(""), ...(look === "Terminal" ? rule('\\[data-ui="b"\\] ') : {}) };
}

/** A colour's hue in degrees, or null for a grey (too little saturation to have one). */
function hueOf(hex: string): number | null {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  if (max - min < 0.2) return null;
  const d = max - min;
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}
const teal = (hex: string) => {
  const h = hueOf(hex);
  return h !== null && h >= 170 && h <= 200;
};

describe("the statement graph's colours", () => {
  for (const [look, tokens] of Object.entries(looks)) {
    it(`in ${look}: a colour per kind, and only a call outside teal`, () => {
      const kinds = kindTokens(look);
      expect(Object.keys(kinds).sort()).toEqual(["answer", "compute", "data", "outside", "waits"]);
      const colours = Object.fromEntries(Object.entries(kinds).map(([k, token]) => [k, tokens[token]]));
      for (const c of Object.values(colours)) expect(c).toMatch(/^#[0-9a-f]{6}$/i);
      expect(new Set(Object.values(colours)).size).toBe(5);
      expect(Object.entries(colours).filter(([k, c]) => k !== "outside" && teal(c))).toEqual([]);
    });
  }
});
