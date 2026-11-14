// Ctrl+K (or /): everything there is to open and do, found by typing a few letters of it, a run
// by its number, and `:` commands typed out (commands.ts). Up and Down pick, Enter does it, Esc
// closes.

import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { COLON_HELP, matches, parseColon, type ColonCommand, type Command } from "../commands";
import { Kbd } from "./Kbd";
import { useFocusTrap } from "./focusTrap";

/** At most this many entries are shown: typing finds the rest. */
const SHOWN = 60;

export function CommandPalette({
  commands,
  byNumber,
  onColon,
  onClose,
}: {
  commands: Command[];
  /** The runs a number typed names: that run of each deployed flow. */
  byNumber?: (seq: number) => Command[];
  /** Do a `:` command; why it couldn't, else null. */
  onColon: (c: ColonCommand) => string | null;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [at, setAt] = useState(0);
  const [why, setWhy] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  useFocusTrap(box, true, () => field.current);
  const list = useId();
  const colon = query.startsWith(":");
  const shown = useMemo(() => (colon ? [] : matches(commands, query, byNumber).slice(0, SHOWN)), [commands, query, colon, byNumber]);
  useEffect(() => {
    setAt(0);
    setWhy("");
  }, [query]);

  const run = (c: Command | undefined) => {
    if (!c) return;
    onClose();
    c.run();
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (shown.length) setAt((i) => (i + (e.key === "ArrowDown" ? 1 : shown.length - 1)) % shown.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (!colon) return run(shown[at]);
      const c = parseColon(query);
      if ("error" in c) return setWhy(c.error);
      const no = onColon(c);
      if (no) setWhy(no);
      else onClose();
    }
  };

  return (
    <div className="hl-modal hl-modal--top" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={box} className="hl-modal__box hl-cmd" role="dialog" aria-modal="true" aria-label="command palette">
        <input
          ref={field}
          className="hl-cmd__field"
          role="combobox"
          aria-label="find a page, a flow or a command"
          aria-expanded="true"
          aria-controls={list}
          aria-activedescendant={!colon && shown[at] ? `${list}-${at}` : undefined}
          placeholder="a page, a flow, an action; : for a command"
          value={query}
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKey}
        />
        {why && (
          <div className="hl-cmd__why" role="status">
            {why}
          </div>
        )}
        {colon ? (
          <ul className="hl-cmd__list" id={list} aria-label="commands">
            {COLON_HELP.map(([c, does]) => (
              <li key={c} className="hl-cmd__item hl-cmd__item--help">
                <span className="hl-cmd__label">{c}</span>
                <span className="hl-cmd__kind">{does}</span>
              </li>
            ))}
          </ul>
        ) : (
          <ul className="hl-cmd__list" id={list} role="listbox" aria-label="matches">
            {shown.length === 0 && <li className="hl-cmd__none">nothing matches</li>}
            {shown.map((c, i) => (
              <li
                key={`${c.kind} ${c.label}`}
                id={`${list}-${i}`}
                role="option"
                aria-selected={i === at}
                className={`hl-cmd__item${i === at ? " hl-cmd__item--at" : ""}`}
                onMouseMove={() => setAt(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => run(c)}
              >
                <span className="hl-cmd__label">{c.label}</span>
                <span className="hl-cmd__kind">{c.kind}</span>
                {i === at && <Kbd k="Enter" />}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
