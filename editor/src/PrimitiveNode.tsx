// A canvas node = a placed instance of a primitive. Its handles are rendered ONLY from the interface
// the catalog gave us (data.inputs / data.output), never hand-placed — that's I1 made structural (see
// P4.1-palette-L4 §2). Each handle's hue is its wire type (theme.ts). Handle ids are the port names
// (inputs) and "out" (output): stable references the wiring inch will turn into Edges by index.

import { memo, type CSSProperties } from "react";
import { Handle, Position, useReactFlow, type NodeProps, type Node } from "@xyflow/react";
import { compatible, type WParam, type WPort, type WType } from "./holon";
import { typeColor, typeLabel, outputColor } from "./theme";

/** What a placed primitive node carries. Mirrors the catalog interface; `position`/`id` (view-only) live on the React Flow node, not here. */
export type PrimitiveData = {
  name: string;
  inputs: WPort[];
  output: WType | null;
  params: WParam[];
};

export type PrimitiveNodeType = Node<PrimitiveData, "primitive">;

/** `const` is the one type-polymorphic atom: its `type` param IS its output type. Core supports
 *  exactly these three literal forms (`primitives.rs` `const`); List/Product/Sum/Effect/Any have no
 *  literal form — a list is built with the `list` constructor, not typed as text. This was always
 *  the true domain of `CONST_TYPES`/`setConstType`; typing them as the full `WType` only ever
 *  type-checked by accident, because the old flat `WType` was a plain string union and TS widens a
 *  narrower string-literal type into `string` for free. Recursive `WType` (P1.3.2 editor inch)
 *  correctly ended that coincidence — a real latent imprecision the compiler just caught, not
 *  something the recursion broke. Narrowing to the true domain fixes it at the root: `const`
 *  genuinely can never be `{ List: ... }`, so the type should never have said it could. */
type ConstType = "Number" | "Boolean" | "Text";
const CONST_TYPES: readonly ConstType[] = ["Number", "Boolean", "Text"];

const fieldStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#cfe3ff",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 4px",
};

function PrimitiveNodeImpl({ id, data }: NodeProps<PrimitiveNodeType>) {
  const { name, inputs, output, params } = data;
  const { updateNodeData, setEdges, getNode } = useReactFlow<PrimitiveNodeType>();

  // Edit a frozen value (text). Core parses it; a bad entry fails soft at eval. The value is in the
  // preimage, so this changes identity — core's `param_value_changes_hash`, now via the canvas.
  const setParam = (pname: string, value: string) => {
    const next = params.map((p) =>
      p.kind === "Frozen" && p.name === pname ? { ...p, value } : p,
    );
    updateNodeData(id, { params: next });
  };

  // `const`'s `type` is also its OUTPUT type. Changing it re-types the node: the param AND the
  // declared output (which re-colours the handle and changes what `compatible` lets it wire into).
  // The editor upholds const's "output == type param" contract here, exactly as the catalog does for
  // the default — it does not decide types; `compatible` stays the sole wire judge. Wires from this
  // output whose target no longer accepts the new type are dropped — fail-closed, the same rule that
  // refused them at connect time; core's `validate` at projection is the real gate behind this.
  const setConstType = (ty: ConstType) => {
    const next = params.map((p) =>
      p.kind === "Frozen" && p.name === "type" ? { ...p, value: ty } : p,
    );
    updateNodeData(id, { params: next, output: ty });
    setEdges((eds) =>
      eds.filter((e) => {
        if (e.source !== id || e.sourceHandle !== "out") return true;
        const dstTy = getNode(e.target)?.data.inputs.find((p) => p.name === e.targetHandle)?.ty;
        return dstTy ? compatible(ty, dstTy) : false;
      }),
    );
  };

  return (
    <div className="hl-node">
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
            const isConstType = name === "const" && p.name === "type";
            return (
              <label className="hl-node__param" key={p.name}>
                {p.name}{" "}
                {isConstType ? (
                  <select
                    className="nodrag"
                    style={fieldStyle}
                    value={p.value}
                    onChange={(e) => setConstType(e.target.value as ConstType)}
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
        </div>

        <div className="hl-node__col hl-node__col--out">
          {output && (
            <div className="hl-port hl-port--out">
              <span className="hl-port__label" style={{ color: outputColor(output) }}>
                {typeLabel(output)}
              </span>
              <Handle
                type="source"
                position={Position.Right}
                id="out"
                className="hl-handle"
                style={{ background: outputColor(output) }}
              />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export const PrimitiveNode = memo(PrimitiveNodeImpl);