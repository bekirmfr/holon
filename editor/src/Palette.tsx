// The palette: every genesis primitive, in catalog (§13a) order, each draggable onto the canvas. The
// list IS primitiveCatalog() — no hand-written TS roster (I8). Dragging carries the primitive name;
// the canvas looks the interface back up from the catalog on drop, so the placed node's shape always
// comes from core. A swatch on each row previews the primitive's output hue. Below them, the saved
// flows: each one placed as a single node (a holon, canvasOps.flowAsNode).

import type { WUnit } from "./holon";
import { outputColor } from "./theme";

/** The MIME key the palette writes and the canvas reads on a drag-and-drop. */
export const DND_MIME = "application/holon-primitive";

/** A dragged saved flow's payload: `flow:<name>` (no primitive or template name has a colon). */
export const FLOW_PREFIX = "flow:";

function nameOf(u: WUnit): string {
  return u.body.kind === "Primitive" ? u.body.name : "?";
}

export function Palette({ catalog, templates, flows }: { catalog: WUnit[]; templates: readonly string[]; flows: string[] }) {
  return (
    <aside className="hl-palette">
      <div className="hl-palette__head" title="saved flows, each placed as one node: the flow as saved now">
        flows
      </div>
      <div className="hl-palette__list" style={{ flex: "none", maxHeight: 160 }}>
        {flows.length === 0 && <div style={{ color: "var(--ink-dim)", fontSize: 11, padding: "2px 8px" }}>save a flow to place it here</div>}
        {flows.map((name) => (
          <div
            key={name}
            className="hl-palette__item"
            draggable
            title={`place "${name}" as one node: the flow as saved now`}
            onDragStart={(e) => {
              e.dataTransfer.setData(DND_MIME, FLOW_PREFIX + name);
              e.dataTransfer.effectAllowed = "move";
            }}
          >
            <span className="hl-palette__swatch" style={{ background: "transparent", border: "1px solid #6fd3e0" }} aria-hidden />
            <span className="hl-palette__name">{name}</span>
          </div>
        ))}
      </div>
      <div className="hl-palette__head">http</div>
      <div className="hl-palette__list" style={{ flex: "none" }}>
        {/* Templates (templates.ts): ready-made groups of the primitives below, not new atoms. */}
        {templates.map((name) => (
          <div
            key={name}
            className="hl-palette__item"
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(DND_MIME, name);
              e.dataTransfer.effectAllowed = "move";
            }}
          >
            <span className="hl-palette__swatch" style={{ background: outputColor("Record") }} aria-hidden />
            <span className="hl-palette__name">{name}</span>
          </div>
        ))}
      </div>
      <div className="hl-palette__head">palette</div>
      <div className="hl-palette__list">
        {catalog.map((u) => {
          const name = nameOf(u);
          return (
            <div
              key={name}
              className="hl-palette__item"
              draggable
              onDragStart={(e) => {
                e.dataTransfer.setData(DND_MIME, name);
                e.dataTransfer.effectAllowed = "move";
              }}
            >
              <span
                className="hl-palette__swatch"
                style={{ background: outputColor(u.outputs?.[0]?.ty) }}
                aria-hidden
              />
              <span className="hl-palette__name">{name}</span>
            </div>
          );
        })}
      </div>
    </aside>
  );
}