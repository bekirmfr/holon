// A canvas node = a placed instance of a primitive. Its handles are rendered ONLY from the interface
// the catalog gave us (data.inputs / data.outputs), never hand-placed — that's I1 made structural
// (see P4.1-palette-L4 §2). Each handle's hue is its wire type (theme.ts). Handle ids are the port
// names — inputs and outputs alike (P4.7: outputs are a Vec<Port>, symmetric with inputs, not a
// single hardcoded "out") — stable references the wiring inch turns into Edges by index.

import { memo, type CSSProperties } from "react";
import { Handle, Position, useReactFlow, type NodeProps, type Node } from "@xyflow/react";
import { compatible, type WParam, type WPort, type WType } from "./holon";
import { typeColor, typeLabel, outputColor, TYPE_COLOR } from "./theme";

/** What a placed primitive node carries. Mirrors the catalog interface; `position`/`id` (view-only) live on the React Flow node, not here.
 *  `variadic`/`minInputs` are P4.1's variadic-arity editing: whether this leaf's port count may
 *  grow past the catalog's declared default, and what that default was (the shrink floor) —
 *  both set once at drop time from core's own `VARIADIC` list, never guessed here. */
/** How a placed node's own identity is determined at project/group time (P4.3.1). A genesis
 *  primitive is rebuilt fresh from its catalog name+interface every time — I3: the same
 *  primitive name always yields the same hash, so "rebuild" and "reference" are the same
 *  operation, exactly how it's always worked. A grouped composite is different: it was frozen
 *  once, when it was grouped, and is referenced by that hash from then on — its own internal
 *  structure is neither rebuilt nor re-inspected from the canvas (that's expand-to-inspect's
 *  job, deliberately a separate, later inch, not this one's). */
export type PlacedBody = { kind: "Primitive" } | { kind: "Composite"; hash: string };

export type PrimitiveData = {
  name: string;
  body: PlacedBody;
  inputs: WPort[];
  outputs: WPort[];
  params: WParam[];
  variadic: boolean;
  minInputs: number;
  /** Purely a rendering flag set by App.tsx's decoratedNodes (see there) after a failed
   *  projection — never set at drop time, never read by onProject/hashing. Optional and
   *  absent on every normal node; present and true only while this node is implicated in
   *  the last projection error. */
  error?: boolean;
};

export type PrimitiveNodeType = Node<PrimitiveData, "primitive">;

/** `const` is the one type-polymorphic atom: its `type` param IS its output type. Core supports
 *  exactly these three literal forms (`primitives.rs`'s shared `parse_literal`, called by both
 *  `const` and `default`); List/Product/Sum/Effect/Any have no literal form — a list is built
 *  with the `list` constructor, not typed as text. This was always the true domain of
 *  `CONST_TYPES`; typing them as the full `WType` only ever type-checked by accident, because
 *  the old flat `WType` was a plain string union and TS widens a narrower string-literal type
 *  into `string` for free. Recursive `WType` (P1.3.2 editor inch) correctly ended that
 *  coincidence — a real latent imprecision the compiler just caught, not something the
 *  recursion broke. Narrowing to the true domain fixes it at the root: a literal genuinely can
 *  never be `{ List: ... }`, so the type should never have said it could.
 *
 *  `CONST_TYPES` itself is shared by both `const`'s and `default`'s own `type` param dropdown —
 *  the same three literal forms, the same list, one definition — and, since this inch, the
 *  same action too: `setLiteralType` retypes the param plus every `Any`-typed input AND
 *  output the node actually declares (zero inputs for `const`, so that pass is a no-op;
 *  `value` for `default`), dropping any wire on either side that no longer fits. Retyping
 *  `default`'s input alongside its output is what keeps the output honest rather than a
 *  guess: once `value`'s own declared type matches, `compatible()` refuses a wrong-typed
 *  source at connect time, the same way it already refused a wrong-typed destination. */
type ConstType = "Number" | "Boolean" | "Text" | "Json";
const CONST_TYPES: readonly ConstType[] = ["Number", "Boolean", "Text", "Json"];
// The wire type a literal produces. A Json literal can be any shape (`[]`, `{...}`), so its port
// is `Any`; the scalar literals produce exactly their own type.
const literalPortType = (ty: ConstType): WType => (ty === "Json" ? "Any" : ty);

// The next port name for a variadic node, continuing the catalog's own single-letter convention
// ("a", "b", …) every current variadic leaf uses. Falls back to a numbered name if that
// convention isn't met — defensive, not expected to trigger for any genesis leaf today, but
// cheaper than assuming the convention holds forever.
function nextPortName(existing: string[]): string {
  const isSingleLetter = existing.length > 0 && existing.every((n) => /^[a-z]$/.test(n));
  if (isSingleLetter) {
    const max = Math.max(...existing.map((n) => n.charCodeAt(0)));
    if (max < "z".charCodeAt(0)) return String.fromCharCode(max + 1);
  }
  return `in${existing.length + 1}`;
}

const fieldStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#cfe3ff",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 4px",
};

const portButtonStyle: CSSProperties = {
  font: "inherit",
  fontSize: 10,
  lineHeight: 1,
  color: "#9fb4d4",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  width: 16,
  height: 16,
  padding: 0,
  cursor: "pointer",
};

function PrimitiveNodeImpl({ id, data }: NodeProps<PrimitiveNodeType>) {
  const { name, inputs, outputs, params, variadic, minInputs, error } = data;
  const { updateNodeData, setEdges, getNode } = useReactFlow<PrimitiveNodeType>();

  // Grow/shrink a variadic node's port count (P4.1: variadic-arity editing). Every port a
  // variadic leaf declares is the same type — add/multiply/and/or/concat/list each have uniform
  // ports — so a new one just copies the last port's type; no type decision is made here. Never
  // shrinks below `minInputs`, the catalog's own declared default (derived at drop time, not a
  // hardcoded floor — see PrimitiveData's own doc for why).
  const addPort = () => {
    const last = inputs[inputs.length - 1];
    const port: WPort = { name: nextPortName(inputs.map((p) => p.name)), ty: last?.ty ?? "Any" };
    updateNodeData(id, { inputs: [...inputs, port] });
  };
  const removePort = () => {
    if (inputs.length <= minInputs) return;
    const removed = inputs[inputs.length - 1];
    updateNodeData(id, { inputs: inputs.slice(0, -1) });
    // Drop any wire that fed the removed port — fail-closed, the same rule setLiteralType
    // already applies when a change makes an existing wire's target port stop existing.
    setEdges((eds) => eds.filter((e) => !(e.target === id && e.targetHandle === removed.name)));
  };

  // Edit a frozen value (text). Core parses it; a bad entry fails soft at eval. The value is in the
  // preimage, so this changes identity — core's `param_value_changes_hash`, now via the canvas.
  const setParam = (pname: string, value: string) => {
    const next = params.map((p) =>
      p.kind === "Frozen" && p.name === pname ? { ...p, value } : p,
    );
    updateNodeData(id, { params: next });
  };

  // The chosen literal type governs every `Any`-typed slot a leaf declares under it — for
  // `const`, that's just its one output (zero inputs, so the input pass below is a no-op,
  // correctly); for `default`, it's both its `value` input AND its output. Narrowing the
  // input alongside the output is what makes retyping the output honest rather than a
  // guess: once `value`'s own declared type matches, `compatible()` — the one function this
  // whole system already uses to judge every wire — refuses a wrong-typed source at connect
  // time, the same way it already refuses a wrong-typed *destination* below. A wire from an
  // `Any`-typed source (e.g. `store_read`) still defers to runtime, exactly like every other
  // `Any` wire in this system already does — this doesn't add or remove that; it only means
  // a *concretely* wrong-typed source can no longer sneak past a claim the output is about to
  // make. Existing wires that no longer fit are dropped on both sides — fail-closed, the same
  // rule that refused them at connect time; core's `validate` at projection is the real gate
  // behind this either way. Exactly one output (P4.7: multi-output is composite-only,
  // permanently) — mapping over `outputs` updates that one entry's type while keeping
  // whatever name it actually has, rather than assuming a literal "out".
  const setLiteralType = (ty: ConstType) => {
    const next = params.map((p) =>
      p.kind === "Frozen" && p.name === "type" ? { ...p, value: ty } : p,
    );
    const portTy = literalPortType(ty);
    const nextInputs = inputs.map((p) => ({ ...p, ty: portTy }));
    const nextOutputs = outputs.map((p) => ({ ...p, ty: portTy }));
    updateNodeData(id, { params: next, inputs: nextInputs, outputs: nextOutputs });

    const outName = outputs[0]?.name;
    const inputNames = new Set(inputs.map((p) => p.name));
    setEdges((eds) =>
      eds.filter((e) => {
        if (e.source === id && e.sourceHandle === outName) {
          // Outgoing: this node's own (newly retyped) output feeding something else.
          const dstTy = getNode(e.target)?.data.inputs.find((p) => p.name === e.targetHandle)?.ty;
          return dstTy ? compatible(portTy, dstTy) : false;
        }
        if (e.target === id && e.targetHandle && inputNames.has(e.targetHandle)) {
          // Incoming: something else feeding one of this node's (newly retyped) inputs.
          const srcTy = getNode(e.source)?.data.outputs.find((p) => p.name === e.sourceHandle)?.ty;
          return srcTy ? compatible(srcTy, portTy) : false;
        }
        return true;
      }),
    );
  };

  return (
    <div
      className="hl-node"
      style={error ? { borderColor: TYPE_COLOR.Effect, boxShadow: `0 0 0 1px ${TYPE_COLOR.Effect}` } : undefined}
    >
      <div className="hl-node__title">{name}</div>

      {params.length > 0 && (
        <div className="hl-node__params">
          {params.map((p) => {
            if (p.kind === "Open") {
              return (
                <span className="hl-node__param" key={p.name}>
                  {p.name} ·
                </span>
              );
            }
            // `const` and `default` both pick their literal from the same three forms
            // (`primitives.rs`'s shared `parse_literal`) — same dropdown, same CONST_TYPES
            // list, and now the same action too: setLiteralType correctly retypes whatever
            // `Any`-typed slots each one actually declares (zero inputs for `const`, one for
            // `default`), so there's no longer a reason to route them differently here.
            const isLiteralTypeParam =
              (name === "const" || name === "default") && p.name === "type";
            return (
              <label className="hl-node__param" key={p.name}>
                {p.name}{" "}
                {isLiteralTypeParam ? (
                  <select
                    className="nodrag"
                    style={fieldStyle}
                    value={p.value}
                    onChange={(e) => setLiteralType(e.target.value as ConstType)}
                  >
                    {CONST_TYPES.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    className="nodrag"
                    style={{ ...fieldStyle, width: 64 }}
                    value={p.value}
                    onChange={(e) => setParam(p.name, e.target.value)}
                  />
                )}
              </label>
            );
          })}
        </div>
      )}

      <div className="hl-node__body">
        <div className="hl-node__col hl-node__col--in">
          {inputs.map((port) => (
            <div className="hl-port" key={port.name}>
              <Handle
                type="target"
                position={Position.Left}
                id={port.name}
                className="hl-handle"
                style={{ background: typeColor(port.ty) }}
              />
              <span
                className="hl-port__label"
                style={{ color: typeColor(port.ty) }}
                title={typeLabel(port.ty)}
              >
                {port.name}
              </span>
            </div>
          ))}
          {variadic && (
            <div style={{ display: "flex", gap: 4, marginTop: 2 }}>
              <button
                className="nodrag"
                onClick={addPort}
                style={portButtonStyle}
                title="add input"
              >
                +
              </button>
              <button
                className="nodrag"
                onClick={removePort}
                disabled={inputs.length <= minInputs}
                style={{ ...portButtonStyle, opacity: inputs.length <= minInputs ? 0.3 : 1 }}
                title="remove last input"
              >
                −
              </button>
            </div>
          )}
        </div>

        <div className="hl-node__col hl-node__col--out">
          {outputs.map((port) => (
            <div className="hl-port hl-port--out" key={port.name}>
              <span className="hl-port__label" style={{ color: outputColor(port.ty) }}>
                {typeLabel(port.ty)}
              </span>
              <Handle
                type="source"
                position={Position.Right}
                id={port.name}
                className="hl-handle"
                style={{ background: outputColor(port.ty) }}
              />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export const PrimitiveNode = memo(PrimitiveNodeImpl);