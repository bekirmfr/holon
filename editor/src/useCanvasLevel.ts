// The canvas's level for the tab shown (canvasLevel.ts; web-console-L4.md §5.2), and the
// Statements level's wiring: the statement graph of the text as typed (drawn again after a pause in
// the typing; a text that doesn't parse keeps the last graph that did, and says so), the cursor's
// line selecting its row, a row picked putting the cursor on its mark, and a row opened showing its
// nodes on the Nodes level, framed. `l` switches. In View runs the graph is the text of the version
// the run ran on (printed from its canvas, as the text pane shows it), with the run drawn on it
// (runRows.ts); its Nodes level is the node canvas with values.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Edge } from "@xyflow/react";
import type { Evaluator } from "./holon";
import { holonGraph, printHolon, type FlowGraph, type GraphRow } from "./holonText";
import { compileRows, keepLevel, lineAt, rowOnCanvas, rowsMay, type CanvasLevel, type RowMay } from "./canvasLevel";
import { graphRun, rowsOnCanvas, type RunOnCanvas } from "./runRows";
import type { GraphRun } from "./StatementGraph";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { TextAsk } from "./TextPane";
import type { PickedRow } from "./inspectText";
import { syncKeyOf, type FlowMeta } from "./tabs";
import { useKeys } from "./ui/Keys";

/** How long typing pauses before the graph is drawn again, as the text pane compiles. */
const PAUSE_MS = 300;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

type Drawn = { tab: number; text: string; graph: FlowGraph | null; error: string | null };

export function useCanvasLevel({
  flow,
  setFlow,
  nodes,
  edges,
  canvasKey,
  ev,
  tab,
  enabled,
  inspecting,
  showText,
  onOpened,
  onPicked,
  run = null,
}: {
  flow: FlowMeta;
  setFlow: (update: (m: FlowMeta) => FlowMeta) => void;
  nodes: PrimitiveNodeType[];
  edges: Edge[];
  /** The canvas's structure (flowDoc.ts's structuralKey): it changes when the structure does. */
  canvasKey: string;
  ev: Evaluator;
  /** The tab shown: a tab shown anew is drawn at once, not after a pause. */
  tab: number;
  /** Whether the editor is shown (its keys are on). */
  enabled: boolean;
  /** Whether the Inspector is open: what each row may do is worked out only for it. */
  inspecting: boolean;
  /** Ask the text pane, opening it first when it is shut. */
  showText: (ask: TextAsk) => void;
  /** A row opened: the canvas's nodes it stands for (none matched: the whole flow), to select
   *  and frame once the Nodes level shows. */
  onOpened: (ids: string[]) => void;
  /** In View runs, a row picked: the canvas's nodes it stands for, for the Inspector. */
  onPicked?: (ids: string[]) => void;
  /** In View runs, the run shown, as replayed on the canvas: drawn on the rows. */
  run?: RunOnCanvas | null;
}) {
  const viewing = flow.viewing !== null;
  const level: CanvasLevel = flow.level;
  const statements = level === "statements";
  const name = useRef(flow.name);
  name.current = flow.name;
  const setLevel = useCallback(
    (l: CanvasLevel) => {
      setFlow((m) => (m.level === l ? m : { ...m, level: l }));
      keepLevel(name.current, l);
    },
    [setFlow],
  );
  useKeys("editor", { l: () => setLevel(statements ? "nodes" : "statements") }, { enabled });

  // The text drawn: as typed while it stands for the canvas (a mistake in it too: the graph then
  // keeps its last), else the canvas printed. In View runs, always the canvas printed: the version
  // shown, as the text pane prints it.
  const key = useMemo(
    () => syncKeyOf(nodes, edges, flow.settings, flow.name),
    // canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [canvasKey, flow.settings, flow.name],
  );
  const typed = !viewing && flow.text.value !== null && flow.text.key === key ? flow.text.value : null;
  const printed = useMemo(() => {
    if (!statements || typed !== null) return null;
    try {
      return printHolon(flow.name, { nodes: nodes as unknown[], edges: edges as unknown[], server: flow.settings }, ev);
    } catch {
      return null;
    }
    // Printed again when the canvas's structure changes, as the text pane prints it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [statements, typed === null, key, ev]);
  const source = typed ?? printed;

  const [drawn, setDrawn] = useState<Drawn | null>(null);
  const drawnNow = useRef(drawn);
  drawnNow.current = drawn;
  // Shown again (another tab, or this one's statements after its nodes), it is drawn at once.
  const fresh = useRef(true);
  useEffect(() => {
    if (!statements) {
      fresh.current = true;
      return;
    }
    const last = drawnNow.current?.tab === tab ? drawnNow.current : null;
    const was = fresh.current ? null : last;
    fresh.current = false;
    if (source === null) {
      setDrawn({ tab, text: last?.text ?? "", graph: last?.graph ?? null, error: "the canvas can't be written as text yet" });
      return;
    }
    if (was && was.text === source && was.error === null) return;
    const draw = () => {
      try {
        setDrawn({ tab, text: source, graph: holonGraph(source), error: null });
      } catch (e) {
        setDrawn({ tab, text: last?.text ?? source, graph: last?.graph ?? null, error: message(e) });
      }
    };
    if (!was) return draw();
    const t = setTimeout(draw, PAUSE_MS);
    return () => clearTimeout(t);
  }, [statements, source, tab]);
  const shown = drawn?.tab === tab ? drawn : null;
  const graph = shown?.graph ?? null;

  // Where the text's cursor is, and the line it is on: its row is selected.
  const [caret, setCaret] = useState<number | null>(null);
  const paneText = !flow.text.open ? null : viewing ? source : flow.text.value;
  const cursorLine = caret !== null && paneText !== null ? lineAt(paneText, caret) : null;

  // The graph's text compiled: which nodes each row stands for, and what each may do.
  const compiled = useMemo(() => (statements && shown?.graph ? compileRows(shown.text, ev) : null), [statements, shown?.graph, shown?.text, ev]);
  const may = useMemo((): Map<string, RowMay[]> | null => (inspecting && graph && compiled ? rowsMay(graph, compiled, ev) : null), [inspecting, graph, compiled, ev]);

  const canvas = useRef({ nodes, edges });
  canvas.current = { nodes, edges };
  // The run on the rows: each row's nodes on the canvas the run was replayed on.
  const rows = useMemo(
    () => (viewing && graph && compiled ? rowsOnCanvas(graph, compiled, canvas.current) : null),
    // canvasKey changes exactly when the canvas's structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [viewing, graph, compiled, canvasKey],
  );
  const drawnRun = useMemo((): GraphRun | null => (rows && run ? graphRun(rows, run) : null), [rows, run]);
  // The row picked last: the Inspector shows it while the cursor is where picking it put it.
  const [picked, setPicked] = useState<PickedRow | null>(null);
  const onPick = useCallback(
    (row: GraphRow, band: number, index: number) => {
      setPicked({ band, index, to: row.at.to });
      showText({ do: "select", from: row.at.from, to: row.at.to });
      if (viewing && compiled) onPicked?.(rowOnCanvas(row, compiled, canvas.current));
    },
    [showText, viewing, compiled, onPicked],
  );
  const onOpen = useCallback(
    (row: GraphRow) => {
      const ids = compiled ? rowOnCanvas(row, compiled, canvas.current) : [];
      setLevel("nodes");
      onOpened(ids);
    },
    [compiled, setLevel, onOpened],
  );

  return {
    level,
    setLevel,
    graph,
    /** In View runs, the run as the rows show it. */
    run: drawnRun,
    /** Why the text drawn has no graph now (it doesn't parse): the last one stays. */
    error: shown?.error ?? null,
    caret,
    setCaret,
    cursorLine,
    paneText,
    picked,
    may,
    onPick,
    onOpen,
  };
}
