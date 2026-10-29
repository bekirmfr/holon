// The text pane and the canvas, one flow (docs/L4s/text-pane-L4.md). The last edit wins: what is
// typed drives the canvas when it compiles (one undo step; nodes it didn't touch keep their place
// and id, textSync.ts), and a canvas change writes the text again from the canvas. Moving a node
// changes nothing in the text: a position is not part of the program. The text as typed is kept
// with the tab (FlowMeta's `text`) and saved with the flow while it stands for the canvas, so its
// comments survive; a canvas change that drops comments says so, and undoing it brings them back.
//
// The two are linked by the range each node is written over (docs/L4s/text-graph-link-L4.md): the
// cursor picks the node under it, a node picked on the canvas marks its range, in view mode too,
// where the text also shows the run: what didn't run dimmed, the step marked, values in notes.

import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import type { Edge } from "@xyflow/react";
import type { Evaluator, WValue } from "./holon";
import type { FlowSettings } from "./DeployBar";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { TextLinked, TextNote } from "./TextPane";
import { valueChip } from "./diagnostics";
import { compileHolon, printHolon, type TextRange } from "./holonText";
import { dimmed, pickAt } from "./textLink";
import { carryOver, commentCount, syncedFromCanvas, type Graph, type Synced } from "./textSync";
import { settingsOf, syncKeyOf, type FlowMeta, type TextState } from "./tabs";

const WIDTH_KEY = "holon.textWidth";
/** How long typing pauses before the text is compiled: for the person, not the machine. */
const PAUSE_MS = 300;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The selected run, as view mode replayed it: the nodes in the order they ran, what each made,
 *  and the step shown. */
export type TextRun = { order: string[]; traces: Map<string, { outputs: (WValue | null)[] }>; step: number | null };

/** A text's ranges and names, by the canvas node each stands for. A node inside a holon's body
 *  stands for none: it is linked through its holon's call. */
type Link = { spans: Map<string, TextRange>; names: Map<string, string> };
function linkOf(spans: Map<string, TextRange>, names: Map<string, string>, toCanvas: Map<string, string>): Link {
  const by = <T,>(m: Map<string, T>) => new Map([...m].flatMap(([t, v]) => (toCanvas.has(t) ? [[toCanvas.get(t)!, v] as const] : [])));
  return { spans: by(spans), names: by(names) };
}

export function useTextPane({
  flow,
  setFlow,
  nodes,
  edges,
  canvasKey,
  live,
  setNodes,
  setEdges,
  ev,
  idRef,
  viewing,
  run,
  onPick,
}: {
  flow: FlowMeta;
  setFlow: (update: (m: FlowMeta) => FlowMeta) => void;
  nodes: PrimitiveNodeType[];
  edges: Edge[];
  /** The canvas's structure (flowDoc.ts's structuralKey): it changes when the structure does. */
  canvasKey: string;
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  setNodes: (update: PrimitiveNodeType[] | ((ns: PrimitiveNodeType[]) => PrimitiveNodeType[])) => void;
  setEdges: (es: Edge[]) => void;
  ev: Evaluator;
  idRef: MutableRefObject<number>;
  viewing: boolean;
  /** In view mode, the run shown; null without one. */
  run: TextRun | null;
  /** In view mode, the node the cursor picked (for the side pane). */
  onPick: (id: string) => void;
}) {
  const text = flow.text;
  const setText = useCallback((update: (t: TextState) => TextState) => setFlow((m) => ({ ...m, text: update(m.text) })), [setFlow]);
  const [status, setStatus] = useState("");
  const [revealSerial, setRevealSerial] = useState(0);
  const [revealId, setRevealId] = useState<string | null>(null);
  const [width, setWidthState] = useState(() => {
    try {
      return Number(localStorage.getItem(WIDTH_KEY)) || 420;
    } catch {
      return 420;
    }
  });
  const setWidth = useCallback((w: number) => {
    setWidthState(w);
    try {
      localStorage.setItem(WIDTH_KEY, String(Math.round(w)));
    } catch {
      // Not remembered, then.
    }
  }, []);

  // What the text stands for, as of this render.
  const key = useMemo(
    () => syncKeyOf(live.current.nodes, live.current.edges, flow.settings, flow.name),
    // canvasKey changes exactly when the structure does; live.current is read on purpose.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [canvasKey, flow.settings, flow.name],
  );
  const now = useRef({ flow, key });
  now.current = { flow, key };

  // Texts as typed, by the canvas they stood for: undoing a canvas change brings its text back.
  const typedTexts = useRef(new Map<string, string>());
  const remember = (k: string, t: string) => {
    typedTexts.current.set(k, t);
    if (typedTexts.current.size > 100) typedTexts.current.delete(typedTexts.current.keys().next().value!);
  };
  // The compiled text that stands for the canvas, and which canvas node each of its nodes is.
  const synced = useRef<{ key: string; synced: Synced; lines: Map<string, number>; spans: Map<string, TextRange> } | null>(null);

  /** The canvas as a document to print or match. */
  const canvasDoc = () => ({ nodes: live.current.nodes as unknown[], edges: live.current.edges as unknown[], server: now.current.flow.settings });

  /** The sync for the canvas as it is: from the text that stands for it, else the canvas printed. */
  const syncedNow = useCallback(() => {
    const { flow: f, key: k } = now.current;
    if (synced.current?.key === k) return synced.current;
    const base = f.text.key === k && f.text.synced !== null ? f.text.synced : printHolon(f.name, canvasDoc(), ev);
    const c = compileHolon(base, ev);
    const canvas: Graph = { nodes: live.current.nodes, edges: live.current.edges };
    synced.current = { key: k, synced: syncedFromCanvas(canvas, { nodes: c.doc.nodes, edges: c.doc.edges } as Graph, c.names), lines: c.lines, spans: c.spans };
    return synced.current;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ev]);

  // The canvas changed (or the pane opened on it): the text is written again from it, or the
  // text typed for it before comes back.
  useEffect(() => {
    if (!text.open || viewing || text.key === key) return;
    const remembered = typedTexts.current.get(key);
    let value: string | null = remembered ?? null;
    if (value === null) {
      try {
        value = printHolon(flow.name, canvasDoc(), ev);
      } catch (e) {
        setStatus(`the canvas can't be written as text yet: ${message(e)}`);
      }
    }
    const before = text.typed ? text.value : null;
    const dropped = before !== null && value !== null && remembered === undefined ? commentCount(before) - commentCount(value) : 0;
    if (value !== null) setStatus(dropped > 0 ? `The canvas change wrote the text again: ${dropped === 1 ? "a comment was" : `${dropped} comments were`} dropped. Undo brings ${dropped === 1 ? "it" : "them"} back.` : "");
    synced.current = null;
    setText((t) => ({ ...t, value, key, synced: value, typed: remembered !== undefined }));
    // Only a new canvas (or opening) writes the text; the text's own fields follow from it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text.open, viewing, key]);

  // What was typed, compiled after a pause: the canvas it stands for, with what it didn't touch
  // where it was.
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const apply = useCallback(
    (typed: string) => {
      const { flow: f, key: k } = now.current;
      let compiled: ReturnType<typeof compileHolon>;
      try {
        compiled = compileHolon(typed, ev);
      } catch (e) {
        setStatus(`${message(e)}. The canvas shows the last version that compiled.`);
        return;
      }
      let prev: Synced;
      try {
        prev = syncedNow().synced;
      } catch {
        prev = { graph: { nodes: [], edges: [] }, names: new Map(), toCanvas: new Map() };
      }
      const next = { graph: { nodes: compiled.doc.nodes, edges: compiled.doc.edges } as Graph, names: compiled.names };
      const out = carryOver(prev, next, live.current.nodes, (base) => `${base}-${idRef.current++}`);
      // A node kept is the canvas's own (its selection, its measured size), with the text's data.
      const old = new Map(live.current.nodes.map((n) => [n.id, n]));
      const ns = out.nodes.map((n) => {
        const o = old.get(n.id);
        return o ? { ...o, data: n.data, position: n.position } : n;
      });
      // The settings the text says (port, every, browsers, optional); the limits it doesn't say
      // are the flow's as they were.
      const said = settingsOf(compiled.doc.server);
      const { optional: _, ...unsaid } = f.settings.requirements ?? {};
      const optional = said.requirements?.optional;
      const requirements = { ...unsaid, ...(optional && optional.length > 0 ? { optional } : {}) };
      const settings: FlowSettings = { port: said.port, interval: said.interval, ...(Object.keys(requirements).length > 0 ? { requirements } : {}), ...(said.browsers ? { browsers: said.browsers } : {}) };
      const k2 = syncKeyOf(ns, out.edges, settings, f.name);
      synced.current = { key: k2, synced: { ...next, toCanvas: out.toCanvas }, lines: compiled.lines, spans: compiled.spans };
      remember(k2, typed);
      setStatus("");
      if (k2 === k) {
        // Only comments or spacing changed: the canvas is as it was.
        setText((t) => ({ ...t, key: k2, synced: typed, typed: true }));
        return;
      }
      setNodes(ns);
      setEdges(out.edges);
      setFlow((m) => ({ ...m, settings, text: { ...m.text, key: k2, synced: typed, typed: true } }));
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ev, syncedNow, setNodes, setEdges, setFlow, setText],
  );

  const onEdit = useCallback(
    (typed: string, formatted: boolean) => {
      setText((t) => ({ ...t, value: typed, typed: true }));
      if (timer.current) clearTimeout(timer.current);
      // Formatted, nothing is half typed: no pause to wait for.
      if (formatted) apply(typed);
      else timer.current = setTimeout(() => apply(typed), PAUSE_MS);
    },
    [apply, setText],
  );
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);

  // A quick fix that rewrites the flow's text (make it runnable by hand): the text as typed, or
  // the canvas written as text, rewritten and applied as a formatted edit is. Throws why the
  // rewrite doesn't apply.
  const rewrite = useCallback(
    (f: (text: string) => string) => {
      const current = text.open && text.typed && text.value !== null && text.key === key ? text.value : printHolon(flow.name, canvasDoc(), ev);
      onEdit(f(current), true);
    },
    // canvasDoc reads live.current, as of the call.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [text.open, text.typed, text.value, text.key, key, flow.name, ev, onEdit],
  );

  // In view mode: the deployed version's text, read only.
  const viewText = useMemo(() => {
    if (!viewing || !text.open) return null;
    try {
      return printHolon(flow.name, { nodes: nodes as unknown[], edges: edges as unknown[], server: flow.settings }, ev);
    } catch (e) {
      return `# the deployed version can't be written as text: ${message(e)}\n`;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing, text.open, canvasKey, flow.name, ev]);

  // The text shown, linked to the canvas: while editing, while it stands for the canvas (not
  // half typed, nor with a mistake); in view mode, the deployed version's text compiled and
  // matched to its canvas as an edit text is (a printed text compiles to the same graph).
  const editLink = useMemo((): Link | null => {
    if (!text.open || viewing || text.key !== key || text.value !== text.synced) return null;
    try {
      const s = syncedNow();
      return linkOf(s.spans, s.synced.names, s.synced.toCanvas);
    } catch {
      return null;
    }
  }, [text.open, viewing, text.key, text.value, text.synced, key, syncedNow]);
  const viewLink = useMemo((): Link | null => {
    if (viewText === null) return null;
    try {
      const c = compileHolon(viewText, ev);
      const s = syncedFromCanvas({ nodes, edges }, { nodes: c.doc.nodes, edges: c.doc.edges } as Graph, c.names);
      return linkOf(c.spans, c.names, s.toCanvas);
    } catch {
      return null;
    }
    // The text is printed from this canvas: they change together.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewText, ev]);
  const link = viewing ? viewLink : editLink;

  // What is picked on the canvas (in view mode, the node in the side pane), and the run, as
  // ranges of the text.
  const selectedIds = viewing ? (flow.inspectNode ?? "") : nodes.filter((n) => n.selected).map((n) => n.id).join(" ");
  const linked = useMemo((): TextLinked | undefined => {
    if (!link) return undefined;
    const rangeOf = (id: string) => link.spans.get(id);
    const marks = selectedIds === "" ? [] : selectedIds.split(" ").flatMap((id) => rangeOf(id) ?? []);
    const reveal = revealId ? (rangeOf(revealId) ?? null) : null;
    if (!viewing || !run) return { marks, dims: [], step: null, reveal, revealSerial, notes: [] };
    const ran = new Set(run.order);
    const atStep = run.step === null ? undefined : run.order[run.step];
    // Notes for what is named (one value each: a route's `req` is a whole request), the node at
    // the step and the one picked: not every node.
    const named = [...link.names.keys()].filter((id) => run.traces.get(id)?.outputs.length === 1);
    const noted = new Set([...named, ...(atStep ? [atStep] : []), ...(flow.inspectNode ? [flow.inspectNode] : [])]);
    const notes: TextNote[] = [...noted].flatMap((id) => {
      const r = rangeOf(id);
      const outputs = run.traces.get(id)?.outputs;
      // A node with no outputs (a write, say) has no value to note.
      if (!r || !ran.has(id) || !outputs || outputs.length === 0) return [];
      const ports = nodes.find((n) => n.id === id)?.data.outputs ?? [];
      const said = outputs.length === 1 ? valueChip(outputs[0] ?? null) : outputs.map((v, i) => `${ports[i]?.name ?? i}: ${valueChip(v ?? null)}`).join("  ");
      return [{ at: r.to, text: said, none: outputs.every((v) => v === null) }];
    });
    return { marks, dims: dimmed(link.spans, (id) => ran.has(id)), step: (atStep && rangeOf(atStep)) || null, reveal, revealSerial, notes };
  }, [link, selectedIds, viewing, run, flow.inspectNode, nodes, revealId, revealSerial]);

  // The cursor picks the node under it: selected on the canvas, or in view mode shown in the
  // side pane (of several written as one, the outermost that ran: lowering makes it last, as the
  // `equals` of a match arm's `"add"` comes after its const).
  const onCursor = useCallback(
    (offset: number) => {
      if (!link) return;
      const ids = pickAt(link.spans, offset);
      if (ids.length === 0) return;
      if (viewing) {
        const ran = ids.filter((id) => run?.order.includes(id));
        onPick(ran[ran.length - 1] ?? ids[ids.length - 1]);
        return;
      }
      const picked = new Set(ids);
      setNodes((ns) => ns.map((n) => (Boolean(n.selected) === picked.has(n.id) ? n : { ...n, selected: picked.has(n.id) })));
    },
    [link, viewing, run, onPick, setNodes],
  );

  const toggle = useCallback(() => setText((t) => ({ ...t, open: !t.open })), [setText]);

  const revealNode = useCallback((id: string) => {
    setRevealId(id);
    setRevealSerial((s) => s + 1);
  }, []);

  return {
    open: text.open,
    toggle,
    rewrite,
    revealNode,
    pane: {
      text: viewing ? (viewText ?? "") : (text.value ?? ""),
      readOnly: viewing,
      onEdit,
      linked,
      onCursor,
      status: viewing ? "" : status,
      width,
      onWidth: setWidth,
      onClose: toggle,
    },
  };
}
