// The editor's Home tab (gaps-web.md G16), at `#editor.home`: a new tab's page. Open a flow
// (saved or deployed), start one, open again a tab closed a moment ago (useTabs's `closed`), and
// the keys the editor answers. With no flow open, the editor shows it.

import { useMemo, useState } from "react";
import { KEYMAP } from "./keymap";
import { follow, format, type Route } from "./routes";
import { Kbd } from "./ui/Kbd";

export type HomeFlow = { name: string; deployed: boolean; saved: boolean; open: boolean };
export type ClosedTab = { name: string | null; kind: "deployed" | "saved" | "draft"; viewing: boolean; dirty: boolean };

export function EditorHome({
  flows,
  closed,
  navigate,
  onNew,
  onReopen,
  onKeys,
}: {
  flows: HomeFlow[];
  /** The tabs closed lately, newest first. */
  closed: ClosedTab[];
  navigate: (r: Route) => void;
  onNew: () => void;
  onReopen: (at: number) => void;
  /** Every key (the ? dialog). */
  onKeys: () => void;
}) {
  const [search, setSearch] = useState("");
  const shown = useMemo(() => {
    const words = search.toLowerCase().split(/\s+/).filter(Boolean);
    return flows.filter((f) => words.every((w) => f.name.toLowerCase().includes(w)));
  }, [flows, search]);
  const keys = KEYMAP.filter((b) => b.scope === "editor" || b.scope === "view");
  const link = (to: Route, text: string, label?: string) => (
    <a href={format(to)} onClick={follow(to, navigate)} aria-label={label}>
      {text}
    </a>
  );
  return (
    <div className="hl-flow hl-edhome">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">Editor</h1>
        <span className="hl-flow__status">a new tab: open a flow, or start one</span>
      </header>
      <div className="hl-edhome__columns">
        <section className="hl-flow__section" aria-label="open a flow">
          <h2>Open a flow</h2>
          <input className="hl-flows__search" type="search" placeholder="Find a flow by name" aria-label="find a flow" value={search} onChange={(e) => setSearch(e.target.value)} />
          {flows.length === 0 && <p className="hl-flow__note">no flow saved or deployed yet</p>}
          <ul className="hl-edhome__list">
            {shown.map((f) => (
              <li key={f.name}>
                {link({ kind: "editor", slug: f.name, view: false, seq: null }, f.name, `edit ${f.name}`)}
                <span className="hl-edhome__said">{[f.deployed ? "deployed" : "", f.saved ? "saved" : "", f.open ? "open" : ""].filter(Boolean).join(" · ")}</span>
                {f.deployed && link({ kind: "editor", slug: f.name, view: true, seq: null }, "view runs", `view ${f.name}'s runs`)}
              </li>
            ))}
          </ul>
          {link({ kind: "flows" }, "every flow, on the Flows page")}
        </section>
        <section className="hl-flow__section" aria-label="start new">
          <h2>Start new</h2>
          <ul className="hl-edhome__list">
            <li>
              <button className="hl-flow__button" onClick={onNew}>
                New draft
              </button>
              <span className="hl-edhome__said">an untitled canvas, named when it is saved</span>
            </li>
            <li>
              {link({ kind: "library", tab: "samples" }, "From a sample")}
              <span className="hl-edhome__said">an unsaved copy of one, from the Library</span>
            </li>
            <li>
              {link({ kind: "library", tab: "tutorials" }, "A tutorial")}
              <span className="hl-edhome__said">the manual's walk-throughs, step by step</span>
            </li>
          </ul>
        </section>
        <section className="hl-flow__section" aria-label="recently closed">
          <h2>Recently closed</h2>
          {closed.length === 0 && <p className="hl-flow__note">no tab closed yet</p>}
          <ul className="hl-edhome__list">
            {closed.map((t, i) => (
              <li key={i}>
                <button className="hl-edhome__reopen" onClick={() => onReopen(i)} aria-label={`reopen ${t.name ?? "untitled"}`}>
                  {t.name ?? "untitled"}
                </button>
                <span className="hl-edhome__said">{[t.kind, t.viewing ? "viewing runs" : "", t.dirty ? "with its unsaved changes" : ""].filter(Boolean).join(" · ")}</span>
              </li>
            ))}
          </ul>
        </section>
        <section className="hl-flow__section" aria-label="keys">
          <h2>Keys</h2>
          <ul className="hl-edhome__keys">
            {keys.map((b) => (
              <li key={b.keys.join(" ")}>
                {b.keys.map((k) => (
                  <Kbd key={k} k={k} />
                ))}{" "}
                {b.does}
              </li>
            ))}
          </ul>
          <button className="hl-flow__button" onClick={onKeys}>
            Every key
          </button>
        </section>
      </div>
    </div>
  );
}
