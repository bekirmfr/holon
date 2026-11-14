// The Library's tutorials (web console L4 §10): the manual's pages that teach, each with its
// first paragraph and the samples it uses, read from the page itself. A card says what its page
// says, and a sample the page names opens from it as an unsaved copy, as the Samples tab does.

import { marked, type Tokens } from "marked";
import { PAGES, contents, plainText } from "./manual";

export type Tutorial = {
  /** The page, by its path inside the manual ("guides/files.md"). */
  path: string;
  title: string;
  /** Its first paragraph, as read. */
  first: string;
  /** The samples it uses, by name, in the order it first names them. */
  samples: string[];
};

export type TutorialSection = { title: string; tutorials: Tutorial[] };

/** A sample as a page may name it: by its name, or its title. */
type Known = { name: string; title: string };

/** Pages that are looked up, not followed: the reference, the language's and its leaves. */
const REFERENCE = /^(reference\/|language\/(reference|leaves)\.md$)/;

/** The tutorials, by the manual's own sections: every page its index lists but the reference. */
export function tutorials(known: Known[]): TutorialSection[] {
  return contents()
    .map((s) => ({
      title: s.title,
      tutorials: s.pages
        .filter((p) => !REFERENCE.test(p.path))
        .map((p) => {
          const text = PAGES.get(p.path) ?? "";
          const samples = [...new Set(sampleMentions(text).map((m) => resolveSample(m, known)).filter((n): n is string => n !== null))];
          return { path: p.path, title: p.title, first: firstParagraph(text), samples };
        }),
    }))
    .filter((s) => s.tutorials.length > 0);
}

/** A page's first paragraph, under its title, as read: its marks without their syntax. */
export function firstParagraph(text: string): string {
  const p = marked.lexer(text).find((t) => t.type === "paragraph") as Tokens.Paragraph | undefined;
  return p ? plainText(p.tokens).replace(/\s+/g, " ").trim() : "";
}

/** The samples a page names, as written, in order: "the **ticker** sample", "the `room` sample",
 *  "the sample **rolling-log**", "the approval-mail sample", "the **billing** and **shop**
 *  samples", and `--sample notes-api` in a command. Each must be a sample (tutorials.test.ts). */
export function sampleMentions(text: string): string[] {
  const out: { at: number; name: string }[] = [];
  const quoted = "(?:\\*\\*|`)([^*`\\n]+)(?:\\*\\*|`)";
  // One or more, joined by "," "and" "or", before "sample(s)".
  for (const m of text.matchAll(new RegExp(`((?:${quoted}(?:,\\s*|\\s+and\\s+|\\s+or\\s+))*)${quoted}\\s+samples?\\b`, "g"))) {
    const names = [...m[0].matchAll(new RegExp(quoted, "g"))].map((x) => x[1]);
    for (const name of names) out.push({ at: m.index, name });
  }
  for (const m of text.matchAll(new RegExp(`\\bsample\\s+${quoted}`, "g"))) out.push({ at: m.index, name: m[1] });
  // A plain name before "sample" is one only when it is a sample's kind of name: words joined by "-".
  for (const m of text.matchAll(/(?<![*`\w-])([a-z0-9]+(?:-[a-z0-9]+)+) samples?\b/g)) out.push({ at: m.index, name: m[1] });
  for (const m of text.matchAll(/--sample ([a-z0-9-]+)/g)) out.push({ at: m.index, name: m[1] });
  return out.sort((a, b) => a.at - b.at).map((m) => m.name);
}

/** Which sample `mention` is: its name, its title, its title before the parenthesis ("Price
 *  rules" of "Price rules (a new version replayed on recent runs)"), or its title as a name. */
export function resolveSample(mention: string, known: Known[]): string | null {
  const said = mention.trim().toLowerCase();
  const asName = said.replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
  const hit = known.find((k) => {
    const title = k.title.toLowerCase();
    return k.name === said || title === said || title.replace(/\s*\(.*\)$/, "") === said || k.name === asName;
  });
  return hit?.name ?? null;
}
