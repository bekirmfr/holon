// Every key the editor answers, in one table: the `?` dialog shows it, and each place that binds
// a key (ui/Keys.tsx's useKeys) binds one listed here, so the list can't fall behind. Browsers
// keep Ctrl+T, Ctrl+W, Ctrl+N and Ctrl+Tab; the editor takes Ctrl+S and Ctrl+D (and later Ctrl+K)
// only where it has a flow to save or deploy.

export type KeyScope = "anywhere" | "editor" | "view" | "review" | "dialog";

export type Binding = {
  /** The keys, as keyOf names them, shown in this order. */
  keys: string[];
  scope: KeyScope;
  does: string;
};

/** The pages the number keys go to, in the side nav's order. */
export const PAGE_KEYS = ["overview", "approvals", "flows", "editor", "library", "resources", "people", "policy"] as const;

/** The step one key away from `step` in a run of `steps` nodes (null: the whole run, which
 *  comes after the last node and before the first, so stepping wraps through it). */
export function stepOf(step: number | null, steps: number, by: -1 | 1): number | null {
  if (step === null) return by > 0 ? 0 : steps - 1;
  const next = step + by;
  return next < 0 || next >= steps ? null : next;
}

export const KEYMAP: Binding[] = [
  { keys: ["1", "…", "8"], scope: "anywhere", does: "go to a page: overview, approvals, flows, editor, library, resources, people, policy" },
  { keys: ["Escape"], scope: "anywhere", does: "go up: a run to its runs, a flow to the flows, a page to the overview (not in the editor)" },
  { keys: ["F6", "shift+F6"], scope: "anywhere", does: "move to the next or previous pane" },
  { keys: ["ctrl+k", "/"], scope: "anywhere", does: "find a page, a flow or an action; : for a command" },
  { keys: ["?"], scope: "anywhere", does: "show these keys" },
  { keys: ["t"], scope: "anywhere", does: "switch the interface: Clean or Terminal" },
  { keys: ["ctrl+s"], scope: "editor", does: "save the flow" },
  { keys: ["ctrl+d"], scope: "editor", does: "deploy it" },
  { keys: ["e"], scope: "editor", does: "edit the flow, or view its runs" },
  { keys: ["c", "x", "i"], scope: "editor", does: "show or hide the canvas, the text, the Inspector" },
  { keys: ["j", "ArrowRight"], scope: "view", does: "step forward one node" },
  { keys: ["k", "ArrowLeft"], scope: "view", does: "step back one node" },
  { keys: ["Home", "End"], scope: "view", does: "the first node, or the whole run" },
  { keys: [",", "."], scope: "view", does: "the run before, or after" },
  { keys: ["v", "b"], scope: "view", does: "show or hide the values beside each output, the runs below" },
  { keys: ["[", "]"], scope: "review", does: "the version waiting before, or after, in the queue" },
  { keys: ["y", "Enter"], scope: "dialog", does: "yes: its button" },
  { keys: ["n", "Escape"], scope: "dialog", does: "no: cancel" },
];
