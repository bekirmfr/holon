// The docs tab: the manual (manual.ts), its contents beside the page shown. Links between its
// pages stay in the tab; links out (a site, a design document) open in a new browser tab. Each
// flow in it (a `holon` block) opens in a tab of its own, as an unsaved copy, to try.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { marked, type Token, type Tokens } from "marked";
import { HOME, PAGES, contents, flowText, plainText, resolve, slugger } from "./manual";

/** A place in the manual: a page, and a heading on it. */
export type ManualAt = { page: string; anchor: string | null };

export const MANUAL_HOME: ManualAt = { page: HOME, anchor: null };

/** A heading's element id: its anchor, kept apart from the editor's own ids. */
const idOf = (anchor: string) => `manual-${anchor}`;

type Ctx = {
  page: string;
  slug: (text: string) => string;
  go: (to: ManualAt) => void;
  tryFlow: (text: string) => void;
};

export function ManualPage({ at: where, onGo, onTry }: {
  /** Where it is; null for the manual's index. */
  at: ManualAt | null;
  onGo: (to: ManualAt) => void;
  /** Opens a flow's text in a tab of its own; why it couldn't, else null. */
  onTry: (text: string) => string | null;
}) {
  const at = where ?? MANUAL_HOME;
  const sections = useMemo(contents, []);
  const tokens = useMemo(() => marked.lexer(PAGES.get(at.page) ?? `# Not found\n\nThe manual has no page \`${at.page}\`.`), [at.page]);
  const [tryError, setTryError] = useState("");
  const top = useRef<HTMLDivElement>(null);

  // To the heading asked for, or the top of the page: the page the tab is in scrolls, and
  // nothing around it.
  useEffect(() => {
    const scroller = top.current?.parentElement;
    if (!scroller) return;
    const el = at.anchor ? document.getElementById(idOf(at.anchor)) : null;
    scroller.scrollTop = el ? scroller.scrollTop + el.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 12 : 0;
  }, [at.page, at.anchor]);
  useEffect(() => setTryError(""), [at.page]);

  // A fresh count of headings each render, so a repeated heading's anchor is the same every time.
  const ctx: Ctx = { page: at.page, slug: slugger(), go: onGo, tryFlow: (text) => setTryError(onTry(text) ?? "") };

  return (
    <div className="hl-manual" ref={top}>
      <nav className="hl-manual__nav" aria-label="manual contents">
        <button className={navClass(at.page === HOME)} onClick={() => onGo(MANUAL_HOME)}>
          The Holon manual
        </button>
        {sections.map((s) => (
          <div key={s.title} className="hl-manual__group">
            <div className="hl-manual__section">{s.title}</div>
            {s.pages.map((p) => (
              <button key={p.path} className={navClass(at.page === p.path)} onClick={() => onGo({ page: p.path, anchor: null })}>
                {p.title}
              </button>
            ))}
          </div>
        ))}
      </nav>
      <article className="hl-manual__doc">
        {tryError && <div className="hl-manual__error">{tryError}</div>}
        {blocks(tokens, ctx)}
      </article>
    </div>
  );
}

const navClass = (active: boolean) => (active ? "hl-manual__link hl-manual__link--active" : "hl-manual__link");

function blocks(tokens: Token[], ctx: Ctx): ReactNode[] {
  return tokens.map((t, i) => block(t, i, ctx));
}

function block(t: Token, key: number, ctx: Ctx): ReactNode {
  switch (t.type) {
    case "heading": {
      const h = t as Tokens.Heading;
      const Tag = `h${h.depth}` as "h1";
      return (
        <Tag key={key} id={idOf(ctx.slug(plainText(h.tokens)))}>
          {inline(h.tokens, ctx)}
        </Tag>
      );
    }
    case "paragraph":
      return <p key={key}>{inline((t as Tokens.Paragraph).tokens, ctx)}</p>;
    case "text": {
      // A tight list item's text: inline, no paragraph around it.
      const x = t as Tokens.Text;
      return <span key={key}>{x.tokens ? inline(x.tokens, ctx) : x.text}</span>;
    }
    case "list": {
      const l = t as Tokens.List;
      const items = l.items.map((item, i) => <li key={i}>{blocks(item.tokens, ctx)}</li>);
      return l.ordered ? (
        <ol key={key} start={typeof l.start === "number" && l.start !== 1 ? l.start : undefined}>
          {items}
        </ol>
      ) : (
        <ul key={key}>{items}</ul>
      );
    }
    case "code": {
      const c = t as Tokens.Code;
      if (c.lang === "holon") {
        return (
          <div key={key} className="hl-manual__flow">
            <pre>
              <code>{c.text}</code>
            </pre>
            <button onClick={() => ctx.tryFlow(flowText(c))} title="open this flow in a tab of its own, as an unsaved copy">
              open in a tab
            </button>
          </div>
        );
      }
      return (
        <pre key={key}>
          <code>{c.text}</code>
        </pre>
      );
    }
    case "table": {
      const tb = t as Tokens.Table;
      const align = (i: number) => tb.align[i] ?? undefined;
      return (
        <div key={key} className="hl-manual__table">
          <table>
            <thead>
              <tr>
                {tb.header.map((c, i) => (
                  <th key={i} style={{ textAlign: align(i) }}>
                    {inline(c.tokens, ctx)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {tb.rows.map((row, r) => (
                <tr key={r}>
                  {row.map((c, i) => (
                    <td key={i} style={{ textAlign: align(i) }}>
                      {inline(c.tokens, ctx)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    }
    case "blockquote":
      return <blockquote key={key}>{blocks((t as Tokens.Blockquote).tokens, ctx)}</blockquote>;
    case "hr":
      return <hr key={key} />;
    case "space":
    case "html":
      // Space between blocks, and the one HTML comment (the page of leaves saying it is generated).
      return null;
    default:
      return <p key={key}>{t.raw}</p>;
  }
}

function inline(tokens: Token[] | undefined, ctx: Ctx): ReactNode[] {
  return (tokens ?? []).map((t, key) => {
    switch (t.type) {
      case "text": {
        const x = t as Tokens.Text;
        return x.tokens ? <span key={key}>{inline(x.tokens, ctx)}</span> : x.text;
      }
      case "escape":
        return (t as Tokens.Escape).text;
      case "strong":
        return <strong key={key}>{inline((t as Tokens.Strong).tokens, ctx)}</strong>;
      case "em":
        return <em key={key}>{inline((t as Tokens.Em).tokens, ctx)}</em>;
      case "del":
        return <del key={key}>{inline((t as Tokens.Del).tokens, ctx)}</del>;
      case "codespan":
        return <code key={key}>{(t as Tokens.Codespan).text}</code>;
      case "br":
        return <br key={key} />;
      case "link":
        return link(t as Tokens.Link, key, ctx);
      default:
        return t.raw;
    }
  });
}

function link(t: Tokens.Link, key: number, ctx: Ctx): ReactNode {
  const to = resolve(ctx.page, t.href);
  if ("url" in to) {
    return (
      <a key={key} href={to.url} target="_blank" rel="noopener noreferrer">
        {inline(t.tokens, ctx)}
      </a>
    );
  }
  return (
    <a
      key={key}
      href={`#${to.anchor ? idOf(to.anchor) : ""}`}
      onClick={(e) => {
        e.preventDefault();
        ctx.go(to);
      }}
    >
      {inline(t.tokens, ctx)}
    </a>
  );
}
