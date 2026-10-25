// The flow's text beside its canvas (docs/L4s/text-pane-L4.md): a CodeMirror editor whose every
// answer is the language's, through the bridge (holonText.ts → holon-author's editing.rs, as
// `holon lsp` gives them): the colours, each mistake where `holon check` says it is, hover,
// completion, and Format as `holon fmt` writes. What the text does to the canvas is App's
// (useTextPane.ts); this is the editor. It also shows what the canvas links to it
// (docs/L4s/text-graph-link-L4.md): the ranges of the nodes picked there, and in view mode the
// run: what didn't run dimmed, the step marked, values in notes beside what made them.

import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Annotation, Compartment, EditorState, RangeSetBuilder, StateEffect, StateField, Transaction } from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  drawSelection,
  highlightActiveLine,
  hoverTooltip,
  keymap,
  lineNumbers,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { autocompletion, completionKeymap, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { linter, lintGutter } from "@codemirror/lint";
import { textColours, textCompletions, textFormatted, textHover, textMistakes, type TextRange } from "./holonText";

/** A change that came from outside the pane (the canvas, a tab switch): not the person typing. */
const outside = Annotation.define<boolean>();

/** Markdown as the language's answers write it (code blocks, `code`, **bold**), into `el`. */
export function markdownInto(el: HTMLElement, md: string) {
  const parts = md.split(/```[a-z]*\n?/);
  parts.forEach((part, i) => {
    if (i % 2 === 1) {
      const pre = document.createElement("pre");
      pre.textContent = part.replace(/\n$/, "");
      el.appendChild(pre);
      return;
    }
    for (const para of part.split(/\n{2,}|\n/).filter((p) => p.trim() !== "")) {
      const p = document.createElement("p");
      for (const bit of para.split(/(`[^`]*`|\*\*[^*]*\*\*)/)) {
        if (bit.startsWith("`") && bit.endsWith("`") && bit.length > 1) {
          const c = document.createElement("code");
          c.textContent = bit.slice(1, -1);
          p.appendChild(c);
        } else if (bit.startsWith("**") && bit.endsWith("**") && bit.length > 3) {
          const b = document.createElement("strong");
          b.textContent = bit.slice(2, -2);
          p.appendChild(b);
        } else if (bit) p.appendChild(document.createTextNode(bit));
      }
      el.appendChild(p);
    }
  });
}

const marks = new Map<string, Decoration>();
const mark = (cls: string) => {
  let m = marks.get(cls);
  if (!m) marks.set(cls, (m = Decoration.mark({ class: `hl-tok-${cls}` })));
  return m;
};

function colour(view: EditorView): DecorationSet {
  const b = new RangeSetBuilder<Decoration>();
  let last = 0;
  for (const c of textColours(view.state.doc.toString())) {
    if (c.to > c.from && c.from >= last) {
      b.add(c.from, c.to, mark(c.class));
      last = c.to;
    }
  }
  return b.finish();
}

const colours = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = colour(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged) this.decorations = colour(u.view);
    }
  },
  { decorations: (v) => v.decorations },
);

/** A value beside the expression that made it, as the wire labels write it: not text, so it
 *  can't be selected, copied or saved. */
export type TextNote = { at: number; text: string; none: boolean };

class NoteWidget extends WidgetType {
  constructor(
    readonly text: string,
    readonly none: boolean,
  ) {
    super();
  }
  eq(other: NoteWidget) {
    return other.text === this.text && other.none === this.none;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = this.none ? "hl-text__value hl-text__value--none" : "hl-text__value";
    el.textContent = this.text;
    return el;
  }
  ignoreEvent() {
    return true;
  }
}

/** What the canvas links to the text: the ranges of the nodes picked there (their lines marked
 *  in the gutter's colour too), and in view mode the run. */
export type TextLinked = { marks: TextRange[]; dims: TextRange[]; step: TextRange | null; notes: TextNote[] };
const NOTHING_LINKED: TextLinked = { marks: [], dims: [], step: null, notes: [] };

const PICKED_LINE = Decoration.line({ class: "hl-text__picked" });
const MARK = Decoration.mark({ class: "hl-text__mark" });
const DIM = Decoration.mark({ class: "hl-text__dim" });
const STEP = Decoration.mark({ class: "hl-text__step" });

const setLinked = StateEffect.define<TextLinked>();
const linked = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(d, tr) {
    for (const e of tr.effects) {
      if (!e.is(setLinked)) continue;
      const doc = tr.state.doc;
      const fits = (r: TextRange) => r.from >= 0 && r.to <= doc.length && r.to > r.from;
      const ds = [];
      for (const r of e.value.marks.filter(fits)) {
        ds.push(MARK.range(r.from, r.to));
        for (let l = doc.lineAt(r.from).number; l <= doc.lineAt(r.to).number; l++) ds.push(PICKED_LINE.range(doc.line(l).from));
      }
      for (const r of e.value.dims.filter(fits)) ds.push(DIM.range(r.from, r.to));
      if (e.value.step && fits(e.value.step)) ds.push(STEP.range(e.value.step.from, e.value.step.to));
      for (const n of e.value.notes) {
        if (n.at >= 0 && n.at <= doc.length) ds.push(Decoration.widget({ widget: new NoteWidget(n.text, n.none), side: 1 }).range(n.at));
      }
      return Decoration.set(ds, true);
    }
    return d.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

const mistakes = linter(
  (view) => {
    const length = view.state.doc.length;
    return textMistakes(view.state.doc.toString()).map((m) => ({
      from: Math.min(m.from, length),
      // A mistake at a point is shown on the character there.
      to: Math.min(Math.max(m.to, m.from + 1), length),
      severity: "error" as const,
      message: m.message,
      source: "holon",
    }));
  },
  { delay: 250 },
);

const hover = hoverTooltip((view, pos) => {
  const said = textHover(view.state.doc.toString(), pos);
  if (!said) return null;
  return {
    pos: said.from,
    end: said.to,
    above: true,
    create() {
      const dom = document.createElement("div");
      dom.className = "hl-text__said";
      markdownInto(dom, said.text);
      return { dom };
    },
  };
});

const TYPE = { keyword: "keyword", function: "function", variable: "variable", module: "namespace", member: "property" } as const;

function complete(ctx: CompletionContext): CompletionResult | null {
  const word = ctx.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/);
  const afterDot = ctx.state.sliceDoc(ctx.pos - 1, ctx.pos) === ".";
  if (!word && !afterDot && !ctx.explicit) return null;
  const offered = textCompletions(ctx.state.doc.toString(), ctx.pos);
  if (offered.length === 0) return null;
  return {
    from: word ? word.from : ctx.pos,
    options: offered.map((o) => ({
      label: o.label,
      type: TYPE[o.kind],
      detail: o.detail,
      info: o.doc
        ? () => {
            const dom = document.createElement("div");
            dom.className = "hl-text__said";
            markdownInto(dom, o.doc!);
            return dom;
          }
        : undefined,
    })),
    validFor: /^[A-Za-z0-9_]*$/,
  };
}

const theme = EditorView.theme(
  {
    "&": { height: "100%", fontSize: "12.5px", background: "var(--bg)", color: "var(--ink)" },
    ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "1.55" },
    ".cm-content": { caretColor: "var(--accent)" },
    ".cm-gutters": { background: "var(--panel-2)", color: "#4d5f7d", border: "none", borderRight: "1px solid var(--line-soft)" },
    ".cm-activeLine": { background: "#ffffff07" },
    ".cm-activeLineGutter": { background: "#ffffff07", color: "var(--ink-dim)" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { background: "#2d4a73 !important" },
    ".cm-tooltip": { background: "var(--panel)", border: "1px solid var(--line)", color: "var(--ink)", borderRadius: "6px" },
    ".cm-tooltip-autocomplete ul li[aria-selected]": { background: "#22406a", color: "var(--ink)" },
    ".cm-completionDetail": { color: "var(--ink-dim)", fontStyle: "normal", marginLeft: "8px" },
    ".cm-diagnostic-error": { borderLeft: "3px solid #e0916f" },
    ".cm-lintRange-error": { backgroundImage: "none", textDecoration: "underline wavy #e0916f", textUnderlineOffset: "3px" },
  },
  { dark: true },
);

/** Format the text as `holon fmt` writes it; false (and the text as it was) when it has a mistake. */
function format(view: EditorView): boolean {
  const text = view.state.doc.toString();
  let formatted: string;
  try {
    formatted = textFormatted(text);
  } catch {
    return false;
  }
  if (formatted !== text) view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: formatted }, userEvent: "format" });
  return true;
}

export function TextPane({
  text,
  readOnly,
  onEdit,
  linked: link = NOTHING_LINKED,
  onCursor,
  status,
  width,
  onWidth,
  onClose,
}: {
  /** The text to show: when it changes from what the pane holds, it replaces it. */
  text: string;
  readOnly: boolean;
  /** Each change the person makes, with the whole text; `formatted` when it is Format's (the
   *  canonical form, nothing half typed in it). */
  onEdit: (text: string, formatted: boolean) => void;
  /** What the canvas links to the text: the ranges of what is picked there, and the run. */
  linked?: TextLinked;
  /** Where the cursor moved to, by the person (an offset). */
  onCursor: (offset: number) => void;
  /** A line above the text: what the canvas shows, what a change cost. */
  status: string;
  width: number;
  onWidth: (w: number) => void;
  onClose: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const editable = useRef(new Compartment());
  const handlers = useRef({ onEdit, onCursor });
  handlers.current = { onEdit, onCursor };
  const [formatError, setFormatError] = useState("");

  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: text,
        extensions: [
          lineNumbers(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          lintGutter(),
          colours,
          // After the colours: a later source's marks wrap an earlier's, so each range is one element.
          linked,
          mistakes,
          hover,
          autocompletion({ override: [complete], icons: false }),
          keymap.of([{ key: "Shift-Alt-f", run: (view) => (setFormatError(format(view) ? "" : "it has a mistake: fix it, then format"), true) }, indentWithTab, ...completionKeymap, ...historyKeymap, ...defaultKeymap]),
          // Read only still takes a cursor: in view mode it picks the node under it.
          editable.current.of(EditorState.readOnly.of(readOnly)),
          EditorView.updateListener.of((u) => {
            if (u.docChanged && !u.transactions.some((t) => t.annotation(outside))) {
              setFormatError("");
              handlers.current.onEdit(u.state.doc.toString(), u.transactions.some((t) => t.isUserEvent("format")));
            }
            if (u.selectionSet && u.view.hasFocus && !u.docChanged) handlers.current.onCursor(u.state.selection.main.head);
          }),
          theme,
          EditorView.contentAttributes.of({ "aria-label": "the flow as text" }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // The editor is made once; what changes reaches it through the effects below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const v = view.current;
    if (!v || v.state.doc.toString() === text) return;
    v.dispatch({
      changes: { from: 0, to: v.state.doc.length, insert: text },
      annotations: [outside.of(true), Transaction.addToHistory.of(false)],
    });
  }, [text]);

  useEffect(() => {
    view.current?.dispatch({ effects: editable.current.reconfigure(EditorState.readOnly.of(readOnly)) });
  }, [readOnly]);

  // After the text: a text replaced takes the ranges that go with it.
  const shownStep = useRef<string>("");
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const at = link.step ? `${link.step.from}:${link.step.to}` : "";
    const scroll = at !== "" && at !== shownStep.current ? [EditorView.scrollIntoView(link.step!.from, { y: "center" })] : [];
    shownStep.current = at;
    v.dispatch({ effects: [setLinked.of(link), ...scroll] });
  }, [link, text]);

  // Dragging the pane's left edge sets its width.
  const onEdge = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = width;
    const move = (m: PointerEvent) => onWidth(Math.max(260, Math.min(window.innerWidth * 0.7, startW + startX - m.clientX)));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const said = formatError || status;
  return (
    <div className="nodrag nowheel hl-text" role="region" aria-label="Text">
      <div className="hl-text__edge" onPointerDown={onEdge} title="drag to resize" />
      <div className="hl-text__head">
        <span className="hl-text__title">Text</span>
        {readOnly ? <span className="hl-text__note">the deployed version, read only</span> : null}
        <span style={{ flex: 1 }} />
        {!readOnly && (
          <button
            className="hl-text__button"
            title="as holon fmt writes it (Shift+Alt+F)"
            onClick={() => {
              if (view.current) setFormatError(format(view.current) ? "" : "it has a mistake: fix it, then format");
            }}
          >
            Format
          </button>
        )}
        <button className="hl-text__button" onClick={onClose} title="close the text" aria-label="close the text">
          ×
        </button>
      </div>
      {said ? (
        <div className="hl-text__status" role="status">
          {said}
        </div>
      ) : null}
      <div className="hl-text__editor" ref={host} />
    </div>
  );
}
