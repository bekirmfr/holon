import { useEffect, useState } from "react";
import { ReactFlow, Background, Controls, type Node, type Edge } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  ensureReady,
  buildPreimage,
  Evaluator,
  BOUNDARY,
  type WUnit,
  type WValue,
} from "./holon";

// The two nodes React Flow draws (purely for visual confirmation; eval does not depend on these).
const nodes: Node[] = [
  { id: "const", type: "input", position: { x: 40, y: 90 }, data: { label: "const(7)" } },
  { id: "out", type: "output", position: { x: 340, y: 90 }, data: { label: "output" } },
];
const edges: Edge[] = [{ id: "const-out", source: "const", target: "out" }];

export default function App() {
  const [result, setResult] = useState<WValue | null | undefined>(undefined);
  const [childHash, setChildHash] = useState<string>("");
  const [error, setError] = useState<string>("");

  useEffect(() => {
    (async () => {
      await ensureReady();
      const ev = new Evaluator();

      // 1. Build const(7) the way the canvas will — a plain object → the one core encoder.
      const konst: WUnit = {
        body: { kind: "Primitive", name: "const" },
        params: [
          { kind: "Frozen", name: "value", value: "7" },
          { kind: "Frozen", name: "type", value: "Number" },
        ],
        inputs: [],
        output: "Number",
      };
      const child = ev.insert(buildPreimage(konst)); // hex hash
      setChildHash(child);

      // 2. Wire it to the composite's boundary output: { children:[const], edges:[const.out → OUT] }.
      const comp: WUnit = {
        body: {
          kind: "Composite",
          children: [child],
          edges: [{ from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: [],
        inputs: [],
        output: "Number",
      };

      // 3. Evaluate the whole graph once, through the one evaluator. No open inputs ⇒ [].
      const out = ev.eval(buildPreimage(comp), []) as WValue | null;
      setResult(out);
    })().catch((e: unknown) => setError(String(e)));
  }, []);

  return (
    <div style={{ fontFamily: "system-ui, sans-serif", padding: 24, maxWidth: 720 }}>
      <h1 style={{ fontSize: 20, marginBottom: 4 }}>Holon editor — end-to-end proof</h1>
      <p style={{ color: "#555", marginTop: 0 }}>
        <code>const(7)</code> wired to the boundary output, evaluated through the wasm bridge.
      </p>

      {error ? (
        <pre style={{ color: "#b00020", whiteSpace: "pre-wrap" }}>{error}</pre>
      ) : (
        <div style={{ fontSize: 18, margin: "12px 0" }}>
          Result:{" "}
          <strong>
            {result === undefined
              ? "…"
              : result === null
                ? "null"
                : result.type === "Number"
                  ? result.value
                  : JSON.stringify(result)}
          </strong>
        </div>
      )}

      <div style={{ fontSize: 12, color: "#888", marginBottom: 16 }}>
        const hash: <code>{childHash ? childHash.slice(0, 16) + "…" : "…"}</code>
      </div>

      <div style={{ height: 320, border: "1px solid #ddd", borderRadius: 8 }}>
        <ReactFlow nodes={nodes} edges={edges} fitView>
          <Background />
          <Controls />
        </ReactFlow>
      </div>
    </div>
  );
}
