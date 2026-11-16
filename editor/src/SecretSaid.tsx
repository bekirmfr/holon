// What a flow may do with a secret, marked wherever "what it may" is listed: a key before the
// words, in the secret's colour (styles.css `--secret`), as the canvas marks the nodes a secret
// reaches. A line of a secret is one a person approving should read first: where it is sent,
// where what is answered to it goes, where a hash or a signature made with it goes, a route
// that answers with it.

import { itemParts } from "./holdings";
import { describeItem } from "./holon";

/** What marks a line of a secret, before its words. */
export const SECRET_MARK = "🔑";

/** Whether a manifest item is of a secret: reading one (and so sending it, or what is made with
 *  it, somewhere), answering its callers with one, reading the server's. */
export function isSecretItem(item: string): boolean {
  return itemParts(item).kind.startsWith("Secret");
}

/** `said` as a line of text shows it: the key first when `item` is of a secret. For where only
 *  text goes (a question's lines, a title). */
export function saidAsText(item: string, said: string): string {
  return isSecretItem(item) ? `${SECRET_MARK} ${said}` : said;
}

/** A question's text, each line that carries the key ([`saidAsText`]) in the secret's colour;
 *  the text itself is as it was given, line ends and all. */
export function SecretLines({ text }: { text: string }) {
  if (!text.includes(SECRET_MARK)) return <>{text}</>;
  return (
    <>
      {text.split("\n").map((line, i) => (
        // Lines of one text, never reordered: the place is the key.
        <span key={i} className={line.includes(SECRET_MARK) ? "hl-secret-said" : undefined}>
          {i > 0 ? "\n" : ""}
          {line}
        </span>
      ))}
    </>
  );
}

/** `item` in words (`said`, or core's own), with the key before it and in the secret's colour
 *  when it is of a secret; as it is otherwise. */
export function Said({ item, said }: { item: string; said?: string }) {
  const text = said ?? describeItem(item);
  if (!isSecretItem(item)) return <>{text}</>;
  return (
    <span className="hl-secret-said">
      <span className="hl-secret-said__key" aria-hidden="true">
        {SECRET_MARK}
      </span>
      {text}
    </span>
  );
}
