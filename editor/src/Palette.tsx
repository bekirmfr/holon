// The palette: every genesis primitive, in catalog (§13a) order, each draggable onto the canvas. The
// list IS primitiveCatalog() — no hand-written TS roster (I8). Dragging carries the primitive name;
// the canvas looks the interface back up from the catalog on drop, so the placed node's shape always
// comes from core. A swatch on each row previews the primitive's output hue.

import type { WUnit } from "./holon";
import { outputColor } from "./theme";

/** The MIME key the palette writes and the canvas reads on a drag-and-drop. */
export const DND_MIME = "application/holon-primitive";

function nameOf(u: WUnit): string {
  return u.body.kind === "Primitive" ? u.body.name : "?";
}

export function Palette({ catalog, templates }: { catalog: WUnit[]; templates: readonly string[] }) {
  return (
    <aside className="hl-palette">
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