// Text the console reads from its own address (the hash's names, a return's query), as it is
// shown: quoted, cut short, on one line, and never as markup. Anyone can make a link, so whatever
// it says is theirs, not the console's: a page that echoes it says so by quoting it (EN:E4).
// fromUrl.test.tsx holds every place that shows such text to it.

/** The most of it shown, in characters, past which it is cut with "…". */
export const FROM_URL_MAX = 80;

// What isn't seen as written: control characters (a new line among them), the format marks that
// reorder or hide text (bidi overrides, zero-width ones), and line and paragraph separators.
const UNSHOWN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]+/gu;
// Quote marks, whose look-alikes would end the console's quote early and go on in its voice.
const QUOTES = /[“”„‟«»‹›〝〞〟＂]/g;

/** `text` on one line, at most FROM_URL_MAX characters: as a label shows a name from the address. */
export function cut(text: string): string {
  const one = text.replace(UNSHOWN, " ").replace(QUOTES, '"').replace(/\s+/g, " ").trim();
  const chars = [...one];
  return chars.length > FROM_URL_MAX ? `${chars.slice(0, FROM_URL_MAX).join("")}…` : one;
}

/** `text` quoted, as a sentence of the console's names what the address said. */
export function quoted(text: string): string {
  return `“${cut(text)}”`;
}

/** `text` quoted in a page: plain text in a `<q>`, marked as the address's. */
export function FromUrl({ text }: { text: string }) {
  return (
    <q data-from-url="" className="hl-from-url">
      {cut(text)}
    </q>
  );
}
