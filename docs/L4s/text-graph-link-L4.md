# The text and the graph, linked: each node's range, in view mode too, with its run (L4)

The text pane is where a flow is written; the canvas is where it is seen. This links them by
what each node is written as, not by line: the cursor in the text picks the node under it,
a node picked on the canvas marks its expression, and the same holds in view mode, where the
text also shows the run: what didn't run is dimmed, the step is marked, and values are written
beside the expressions that made them. Written 2026-10-25, before the code; built the same day
(`editor/src/textLink.ts`, `useTextPane.ts`, `TextPane.tsx`; `compile_holon`'s `spans`). Where
the build differs from the plan, the decision says so (*Built:*). Builds on the text pane
(docs/L4s/text-pane-L4.md) and view mode's replay (`editor/src/replay.ts`).

## Where it stands

- **Every node has a span.** Lowering records where each node it makes is written
  (`Lowered.spans`, node id → `Span`, in `lang/lower.rs`), nodes inside holon bodies included.
- **The bridge passes a line, not a range.** `compile_holon` turns the spans into the start line
  of each node (`lines`), and `print_holon_lines` gives the line each top-level node is printed
  on; the printer knows lines only (each printed line carries the ids written on it).
- **Selection is linked by line, in edit mode only.** A node selected on the canvas marks its
  line in the text, and the cursor on a line selects every node written on it
  (`useTextPane.ts`, `lines` and `onCursorLine`). On `respond { signature: hmac_sha256(…),
  body_base64: base64(req.body) }` the cursor anywhere selects all of them. While viewing a
  run, `lines` is empty: the text shown there (the deployed version, printed) isn't linked.
- **View mode shows the run on the canvas only.** It replays the selected run in the browser
  (`replay`: `traces`, each node's outputs; `notRun`, each node that didn't run and why), dims
  what didn't run, labels a wire with its value on hover (`valueChip`), steps through the run in
  order, and the Inspector says why a node didn't run. The text pane shows none of it.

## What it is

1. **The cursor picks a node, a node marks its expression.** In the text, the cursor selects the
   innermost node written over it: on `base64(req.body)` inside that `respond`, the cursor on
   `base64` selects `base64`, on `body` the field node. A node selected on the canvas marks its
   range in the text (the gutter still marks its line). Several selected, several marked.
2. **The same link while viewing a run.** The deployed version's text, read only, links to the
   deployed canvas exactly as the edit text links to the edit canvas.
3. **The run, in the text.** With a run selected:
   - **What didn't run is dimmed**, the way the canvas dims it: the arms of a `match` not
     chosen, the branch of an `if` not taken, the right side of a `??` not needed. *Built:* of
     a `match` arm not chosen it is the value that dims (`a + b`), not its pattern: the
     comparison with `"add"` ran, to find it wasn't the one. A route the run wasn't for dims
     whole: none of it ran.
   - **The step is marked.** Stepping through the run (the bottom pane's timeline, the step
     buttons) marks the range of the node at that step, and scrolls to it.
   - **Values beside expressions.** A small note after an expression says what it made, in the
     words of the wire labels (`valueChip`: `42`, `"ada"`, `none`, `—` for no value): always
     for each name bound (`let`, `node`) and for the node at the step, and for the node under
     the cursor. Not for every node: a line of notes is unreadable.

## Decisions

1. **Ranges cross the bridge as UTF-16 offsets.** `compile_holon` answers `spans`, each node id to
   `{from, to}` in the offsets CodeMirror counts, converted by the bridge's `Offsets` as
   mistakes and hover already are; `lines` stays for its current readers (the Inspector's
   source line). Columns count characters in the language, offsets count UTF-16 units in the
   editor: a string with an emoji before a node is the test that they agree.
2. **The printer stays line-based; view mode compiles what it printed.** Giving the printer
   ranges means tracking offsets through every expression it writes. Instead, view mode compiles
   the text it printed (a printed text compiles to the same graph: the text's hash is its
   graph's) and matches it to the deployed canvas with `syncedFromCanvas`, as the edit pane
   matches a typed text to its canvas. One mapping, one code path, both modes. It is computed
   once per version shown, not per keystroke.
3. **Innermost wins, ties together.** Spans nest: an expression's node spans its arguments'. The
   cursor picks the nodes with the smallest span containing it; nodes with that same span (the
   several nodes one `match` arm or one `when` lowers to) are picked together. A node lowering
   made with no text of its own (a `const true` for a `when`) has its expression's span and is
   picked with it. *Built:* in view mode, where the side pane shows one node, it shows the
   outermost of them that ran (lowering makes it last: a pattern's `equals` after its const).
4. **Dimmed by the innermost node.** A character is dimmed when the innermost node written over it
   didn't run. So in `if ok then fetch(url) else cached`, with `ok` false, `fetch(url)` dims and
   `if ok then … else cached` doesn't: the choice ran, its untaken arm didn't. Characters no node
   is written over (keywords between statements, comments) are never dimmed.
5. **Values are CodeMirror widgets, not text.** A note is a `Decoration.widget` at the end of
   its node's range, styled like the wire labels (mono, small, muted green, grey for `—`), so
   it can't be selected, copied or saved, and a read-only text stays exactly the deployed text.
   A node with several outputs shows each, as `name: value`. *Built:* the read-only text is
   still focusable (CodeMirror's `readOnly`, no longer `editable: false`), so it takes a cursor.
6. **Linked only while the text stands for the canvas.** As today: in edit mode the link is off
   while the text has mistakes or hasn't been applied yet (`text.value !== text.synced`), so a
   half-typed line never selects a stale node. In view mode the text always stands for the
   version shown.
7. **Top-level nodes first; inside a holon later.** Nodes inside a holon body have spans, but
   under the holon's own ids, and view mode steps into a holon through `innerSteps`. Linking a
   holon's body text to its inner steps is a second inch, once the first is in use. Until then
   the cursor inside a holon's definition picks nothing, and a holon's call picks its node.
8. **The pane opens by default.** Text is the easier way in, so a tab opens with the text pane
   open; the toggle still closes it, per tab, as now. In view mode it opens beside the run.
   *Built:* where the window is 1600px wide or more (`TEXT_OPEN_WIDTH`). Narrower, the pane
   beside the Inspector and the palette left the canvas too narrow to work on (at 1280, a
   selected node's toolbar had no room to show), so there it opens on asking, as before.

## What doesn't change

The language, the printer's output, units and hashes: this reads spans lowering already keeps
and runs view mode already replays. Nothing is sent to the server. No new sample flow: this is
how the editor shows any flow, and the samples that show it best already exist (`calc`, whose
`match` arms dim; `cache-aside`, whose fetch dims when the name is cached).

## Tests

- **The bridge** (holon-wasm): `compile_holon`'s `spans` are UTF-16 offsets (a string with an
  emoji before a node), nest (each argument's range inside its call's), and cover every node of
  the document.
- **Picking** (vitest, against the real core): the cursor on each part of `respond { signature:
  hmac_sha256(secret.signing_key, req.body), body_base64: base64(req.body) }` picks the node
  under it; a `match` arm picks all its nodes; a canvas selection marks those ranges.
- **Dimming** (vitest): `calc` replayed for `{"op": "div", "a": 1, "b": 0}` dims exactly `a + b`,
  `a - b` and `a * b`; the patterns, the `div` arm and the `match` aren't dimmed, and the divide
  ran and made no value (so the route answered 400).
- **e2e**: deploy `calc`, send it a request, open the run in view mode: the text pane shows the
  arms not chosen dimmed, stepping marks each node in turn, the cursor on the `add` arm selects
  its nodes on the canvas, and the notes on `a` and `b` read the values the run was sent.

## What ships

- holon-wasm: `spans` from `compile_holon`.
- editor: ranges in `useTextPane` (edit and view), picking and marking by range in `TextPane`,
  the view-mode mapping (compile the printed text, `syncedFromCanvas`), dimming, the step mark
  and value notes as decorations, the pane open by default.
- docs/L4s/text-pane-L4.md: a line pointing here.
