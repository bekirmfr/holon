// The Grouped level's frames (statementGroups.ts), drawn in the canvas's own coordinates under its
// nodes: a trigger's round its statements, each statement's round its nodes, with its kind word,
// line and text in the bar along its top, in its kind's colour as the Statements level draws it.

import { ViewportPortal } from "@xyflow/react";
import type { Frame } from "./statementGroups";

export function StatementFrames({ frames }: { frames: Frame[] }) {
  return (
    <ViewportPortal>
      {frames.map((f) => (
        <div
          key={f.key}
          className={[
            "hl-frame",
            `hl-frame--${f.level}`,
            f.kind ? `hl-sg--${f.kind}` : "",
            f.head ? "hl-frame--head" : "",
          ]
            .filter(Boolean)
            .join(" ")}
          style={{ transform: `translate(${f.x}px, ${f.y}px)`, width: f.width, height: f.height }}
        >
          <div className="hl-frame__bar" title={f.text || undefined}>
            <span className="hl-frame__label">{f.label}</span>
            {f.text && <span className="hl-frame__text">{f.text}</span>}
          </div>
        </div>
      ))}
    </ViewportPortal>
  );
}
