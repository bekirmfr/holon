// The mouse on a canvas, the same in Edit and in View runs, on the flow and inside a composite:
// the middle button held pans; a left click selects; a node dragged moves (where nodes move);
// and a left drag on the empty canvas draws a box, selecting every node wholly inside it. The wheel
// zooms, as React Flow has it, and Space held makes a left drag pan too.

import { SelectionMode, type ReactFlowProps } from "@xyflow/react";
import type { MouseEvent } from "react";

/** React Flow's mouse buttons: 1 is the middle. */
const MIDDLE = 1;

export const GESTURES = {
  panOnDrag: [MIDDLE],
  selectionOnDrag: true,
  // Only a node the box holds whole, not one it only touches.
  selectionMode: SelectionMode.Full,
  // The middle button's press is the pan's, not the browser's autoscroll (Windows draws its
  // scroll cursor and scrolls the page under it).
  onMouseDown: (e: MouseEvent) => {
    if (e.button === MIDDLE) e.preventDefault();
  },
} satisfies Partial<ReactFlowProps>;
