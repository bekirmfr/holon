// The manual (docs/manual), built into the editor: its pages as written, its contents as its own
// index (README.md) lists them, and its links followed inside the docs page. Nothing here is a
// copy: the pages are read from the repository at build time, so the editor shows the manual the
// commit has. manual.test.ts checks every link lands and every flow in it opens.

import { marked, type Token, type Tokens } from "marked";

/** Every page, by its path inside the manual ("guides/files.md"), as written. */
export const PAGES: ReadonlyMap<string, string> = new Map(
  Object.entries(import.meta.glob("../../docs/manual/**/*.md", { query: "?raw", import: "default", eager: true }) as Record<string, string>).map(
    ([file, text]) => [file.replace(/^.*\/docs\/manual\//, ""), text.replace(/\r\n/g, "\n")],
  ),
);

/** The page the docs open on: the manual's index. */
export const HOME = "README.md";

/** Where a page outside the manual is read: the repository, on GitHub. */
const REPO = "https://github.com/bekirmfr/holon/blob/main/";

export type Section = { title: string; pages: { path: string; title: string }[] };

/** A page's title: its first heading. */
export function titleOf(path: string): string {
  const text = PAGES.get(path) ?? "";
  return /^# (.+)$/m.exec(text)?.[1].trim() ?? path;
}

/** The manual's contents, as its index lists them: each `##` section with the pages it links. */
export function contents(): Section[] {
  const sections: Section[] = [];
  for (const t of marked.lexer(PAGES.get(HOME) ?? "")) {
    if (t.type === "heading" && t.depth === 2) sections.push({ title: t.text, pages: [] });
    if (t.type !== "list" || sections.length === 0) continue;
    const at = sections[sections.length - 1];
    for (const item of (t as Tokens.List).items) {
      const link = firstLink(item.tokens);
      const to = link && resolve(HOME, link.href);
      if (to && "page" in to) at.pages.push({ path: to.page, title: link.text.replace(/\*/g, "") });
    }
  }
  return sections.filter((s) => s.pages.length > 0);
}

function firstLink(tokens: Token[] | undefined): Tokens.Link | null {
  for (const t of tokens ?? []) {
    if (t.type === "link") return t as Tokens.Link;
    const inner = firstLink((t as { tokens?: Token[] }).tokens);
    if (inner) return inner;
  }
  return null;
}

/** Where a link on page `from` goes: a page of the manual (and a heading on it), or a web
 *  address (a site, or a file of the repository outside the manual). */
export function resolve(from: string, href: string): { page: string; anchor: string | null } | { url: string } {
  if (/^[a-z]+:/i.test(href)) return { url: href };
  const [path, anchor = null] = href.split("#", 2);
  if (path === "") return { page: from, anchor };
  // As a path in the repository, from the page's own place in it.
  const parts = `${MANUAL}${from}`.split("/").slice(0, -1);
  for (const p of path.split("/")) {
    if (p === "..") parts.pop();
    else if (p !== ".") parts.push(p);
  }
  const file = parts.join("/");
  if (file.startsWith(MANUAL)) return { page: file.slice(MANUAL.length), anchor };
  return { url: `${REPO}${file}${anchor ? `#${anchor}` : ""}` };
}

/** Where the manual is in the repository. */
const MANUAL = "docs/manual/";

/** A heading's anchor, as GitHub makes it (so the manual's links work in both places): lower
 *  case, punctuation dropped, spaces as `-`; a repeat takes `-1`, `-2`, … */
export function slugger(): (text: string) => string {
  const seen = new Map<string, number>();
  return (text) => {
    const base = text
      .trim()
      .toLowerCase()
      .replace(/[^\p{L}\p{N}\s_-]/gu, "")
      .replace(/\s/g, "-");
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return n === 0 ? base : `${base}-${n}`;
  };
}

/** A heading's text as read: its inline marks (code, emphasis, links) without their syntax. */
export function plainText(tokens: Token[] | undefined): string {
  return (tokens ?? [])
    .map((t) => ("tokens" in t && t.tokens ? plainText(t.tokens) : "text" in t ? (t as { text: string }).text : ""))
    .join("");
}

/** Every heading anchor on a page. */
export function anchorsOf(path: string): Set<string> {
  const slug = slugger();
  const out = new Set<string>();
  const walk = (ts: Token[]) => {
    for (const t of ts) {
      if (t.type === "heading") out.add(slug(plainText((t as Tokens.Heading).tokens)));
      if (t.type === "blockquote") walk((t as Tokens.Blockquote).tokens);
    }
  };
  walk(marked.lexer(PAGES.get(path) ?? ""));
  return out;
}

/** The flows on a page: its `holon` blocks, each whole (the manual's convention). */
export function flowsOf(path: string): string[] {
  const out: string[] = [];
  const walk = (ts: Token[] | undefined) => {
    for (const t of ts ?? []) {
      if (t.type === "code" && (t as Tokens.Code).lang === "holon") out.push(flowText(t as Tokens.Code));
      if (t.type === "list") for (const item of (t as Tokens.List).items) walk(item.tokens);
      if (t.type === "blockquote") walk((t as Tokens.Blockquote).tokens);
    }
  };
  walk(marked.lexer(PAGES.get(path) ?? ""));
  return out;
}

/** A `holon` block's text as a file holds it. */
export const flowText = (t: Tokens.Code) => t.text + "\n";
