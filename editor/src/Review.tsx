// A review in hunks (hunks.ts): a header row for each, ▾ open or ▸ closed with its summary, then
// its rows with their line numbers and a sign, grounds by what each does. Keys while it has the
// focus: n / N the next or previous hunk that changes something, space opens or closes it, w the
// whole text (every fold open), d back to the diff. A deploy's consent shows one (useDeployFlow.ts).

import { useRef, useState, type KeyboardEvent } from "react";
import { isFold, nextHunk, type Hunk, type Row } from "./hunks";

export function Review({ hunks, label = "what changes" }: { hunks: Hunk[]; label?: string }) {
  const [open, setOpen] = useState(() => hunks.map((h) => h.opens));
  const [whole, setWhole] = useState(false);
  const [at, setAt] = useState(() => Math.max(0, hunks.findIndex((h) => h.changes)));
  const heads = useRef<(HTMLButtonElement | null)[]>([]);
  const toggle = (i: number) => setOpen((o) => o.map((v, k) => (k === i ? !v : v)));
  const go = (i: number) => {
    setAt(i);
    setOpen((o) => o.map((v, k) => (k === i ? true : v)));
    heads.current[i]?.focus();
    heads.current[i]?.scrollIntoView({ block: "nearest" });
  };
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "n" || e.key === "N") go(nextHunk(hunks, at, e.key === "n" ? 1 : -1));
    else if (e.key === " ") toggle(at);
    else if (e.key === "w") setWhole(true);
    else if (e.key === "d") setWhole(false);
    else return;
    e.preventDefault();
  };

  return (
    <div className="hl-review" role="group" aria-label={label} tabIndex={0} data-own-keys onKeyDown={onKey} aria-keyshortcuts="N Shift+N Space W D">
      {hunks.map((h, i) => (
        <section key={`${i} ${h.title}`} className="hl-review__hunk" role="region" aria-label={h.title}>
          <button
            ref={(el) => {
              heads.current[i] = el;
            }}
            className={`hl-review__head${i === at ? " hl-review__head--at" : ""}`}
            aria-expanded={open[i]}
            onClick={() => {
              setAt(i);
              toggle(i);
            }}
            onFocus={() => setAt(i)}
          >
            <span aria-hidden="true">{open[i] ? "▾" : "▸"}</span>
            <span className="hl-review__title">{h.title}</span>
            <span className="hl-review__summary">{h.summary}</span>
          </button>
          {/* Closed, its rows stay in the page, hidden: what it says is still found. */}
          <div className="hl-review__rows" hidden={!open[i]}>
            {h.rows.map((r, k) =>
              isFold(r) ? (
                whole ? (
                  r.fold.map((f, j) => <RowLine key={`${k}.${j}`} row={f} />)
                ) : (
                  <button key={k} className="hl-review__fold" onClick={() => setWhole(true)} title="show the whole text (w)">
                    ⋯ {r.fold.length} unchanged line{r.fold.length === 1 ? "" : "s"}
                  </button>
                )
              ) : (
                <RowLine key={k} row={r} />
              ),
            )}
          </div>
        </section>
      ))}
    </div>
  );
}

function RowLine({ row }: { row: Row }) {
  return (
    <div className={`hl-review__row hl-review__row--${row.tone}`}>
      <span className="hl-review__n">{row.n ?? ""}</span>
      <span className="hl-review__sign">{row.sign}</span>
      <span className="hl-review__text">{row.text}</span>
      {row.note && <span className="hl-review__note">{row.note}</span>}
    </div>
  );
}
