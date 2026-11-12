// Every key the editor answers, in one table: the `?` dialog shows it, and each place that binds
// a key (ui/Keys.tsx's useKeys and keysFor) binds one listed here for its scope, which the types
// hold it to, so the list can't fall behind (keymap.test.ts checks the other way: each key listed
// is bound somewhere). Browsers keep Ctrl+T, Ctrl+W, Ctrl+N and Ctrl+Tab; the editor takes Ctrl+S
// and Ctrl+D only where it has a flow to save or deploy.
//
// A page's keys come before the anywhere keys: `d` is the docs anywhere, the diff in a review,
// decline on Approvals.

export type KeyScope = "anywhere" | "list" | "tabs" | "flow" | "run" | "approvals" | "editor" | "view" | "review" | "dialog";

export type Binding = {
  /** The keys, as keyOf names them, shown in this order (a run of digits as "1 to 8"). */
  keys: readonly string[];
  scope: KeyScope;
  does: string;
};

/** The step one key away from `step` in a run of `steps` nodes (null: the whole run, which
 *  comes after the last node and before the first, so stepping wraps through it). */
export function stepOf(step: number | null, steps: number, by: -1 | 1): number | null {
  if (step === null) return by > 0 ? 0 : steps - 1;
  const next = step + by;
  return next < 0 || next >= steps ? null : next;
}

export const KEYMAP = [
  { keys: ["1", "2", "3", "4", "5", "6", "7", "8"], scope: "anywhere", does: "go to a page: overview, approvals, flows, editor, library, resources, people, policy" },
  { keys: ["s", "d"], scope: "anywhere", does: "Getting started; the docs" },
  { keys: ["Escape"], scope: "anywhere", does: "go up: a run to its runs, a flow to the flows, a page to the overview (not in the editor)" },
  { keys: ["F6", "shift+F6"], scope: "anywhere", does: "move to the next or previous pane" },
  { keys: ["ctrl+k", "/"], scope: "anywhere", does: "find a page, a flow, a run or an action; : for a command" },
  { keys: ["?"], scope: "anywhere", does: "show these keys" },
  { keys: ["t"], scope: "anywhere", does: "switch the interface: Clean or Terminal" },
  { keys: ["j", "k"], scope: "list", does: "move the selection down or up (each page keeps its own): Flows, a flow's runs, Approvals" },
  { keys: ["Enter"], scope: "list", does: "open the one selected" },
  { keys: ["[", "]"], scope: "tabs", does: "the tab before, or after: a flow's, the editor's, Approvals'" },
  { keys: ["e"], scope: "flow", does: "on a flow's page: open it in the editor" },
  { keys: [",", "."], scope: "run", does: "on a run's page: the run before, or after" },
  { keys: ["a"], scope: "approvals", does: "approve the one selected: a version opens its review first, a run's approval asks first" },
  { keys: ["d"], scope: "approvals", does: "decline the one selected, with a reason (Approvals' d, not the docs)" },
  { keys: ["ctrl+s"], scope: "editor", does: "save the flow" },
  { keys: ["ctrl+d"], scope: "editor", does: "deploy it" },
  { keys: ["e"], scope: "editor", does: "edit the flow, or view its runs" },
  { keys: ["c", "x", "i", "b"], scope: "editor", does: "show or hide the canvas, the text, the Inspector, the bottom pane" },
  { keys: ["l"], scope: "editor", does: "the canvas's level: its statements, or its nodes" },
  { keys: ["j", "ArrowRight"], scope: "view", does: "step forward one node" },
  { keys: ["k", "ArrowLeft"], scope: "view", does: "step back one node" },
  { keys: ["Home", "End"], scope: "view", does: "the first node, or the whole run" },
  { keys: [",", "."], scope: "view", does: "the run before, or after" },
  { keys: ["v"], scope: "view", does: "show or hide the values beside each output" },
  { keys: ["[", "]"], scope: "review", does: "the version waiting before, or after, in the queue" },
  { keys: ["n", "N"], scope: "review", does: "the next, or previous, part that changes something" },
  { keys: ["Space"], scope: "review", does: "open or close the part" },
  { keys: ["w", "d"], scope: "review", does: "the whole flow; the diff (the review's d, not the docs)" },
  { keys: ["Enter"], scope: "dialog", does: "its highlighted button" },
  { keys: ["Escape"], scope: "dialog", does: "cancel, or close" },
  { keys: ["y", "n"], scope: "dialog", does: "yes or no; any other button, the letter shown on it" },
] as const satisfies readonly Binding[];

/** The keys KEYMAP lists for `scope`: all a binding there may take. */
export type KeyIn<S extends KeyScope> = Extract<(typeof KEYMAP)[number], { scope: S }>["keys"][number];

/** A letter for each button, shown on it and pressed for it: the first of its label's letters
 *  (its words' first letters before the rest) not already another's or `taken`'s. A button
 *  whose letters are all taken has none (""). */
export function lettersOf(labels: string[], taken: string[] = []): string[] {
  const used = new Set(taken.map((t) => t.toLowerCase()));
  return labels.map((label) => {
    const words = label.toLowerCase().split(/[^a-z]+/).filter(Boolean);
    const order = [...words.map((w) => w[0]), ...words.join("")];
    const letter = order.find((c) => !used.has(c)) ?? "";
    if (letter) used.add(letter);
    return letter;
  });
}
