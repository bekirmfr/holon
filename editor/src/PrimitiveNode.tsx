// A canvas node = a placed instance of a primitive. Its handles are rendered ONLY from the interface
// the catalog gave us (data.inputs / data.output), never hand-placed — that's I1 made structural (see
// P4.1-palette-L4 §2). Each handle's hue is its wire type (theme.ts). Handle ids are the port names
// (inputs) and "out" (output): stable references the wiring inch will turn into Edges by index.

import { memo } from "react";
import { Handle, Position, type NodeProps, type Node } from "@xyflow/react";
import type { WParam, WPort, WType } from "./holon";
import { TYPE_COLOR, outputColor } from "./theme";

/** What a placed primitive node carries. Mirrors the catalog interface; `position`/`id` (view-only) live on the React Flow node, not here. */
export type PrimitiveData = {
  name: string;
  inputs: WPort[];
  output: WType | null;
  params: WParam[];
};

export type PrimitiveNodeType = Node<PrimitiveData, "primitive">;

function paramSummary(p: WParam): string {
  return p.kind === "Frozen" ? `${p.name} ${p.value}` : `${p.name} ·`;
}

function PrimitiveNodeImpl({ data }: NodeProps<PrimitiveNodeType>) {
  const { name, inputs, output, params } = data;

  return (
    <div className="hl-node">
      <div className="hl-node__title">{name}</div>

      {params.length > 0 && (
        <div className="hl-node__params">
          {params.map((p) => (
            <span className="hl-node__param" key={p.name}>
              {paramSummary(p)}
            </span>
          ))}
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
                style={{ background: TYPE_COLOR[port.ty] }}
              />
              <span className="hl-port__label" style={{ color: TYPE_COLOR[port.ty] }}>
                {port.name}
              </span>
            </div>
          ))}
        </div>

        <div className="hl-node__col hl-node__col--out">
          {output && (
            <div className="hl-port hl-port--out">
              <span className="hl-port__label" style={{ color: outputColor(output) }}>
                {output.toLowerCase()}
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
