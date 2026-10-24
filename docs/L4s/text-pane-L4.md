# The web editor's text pane (L4)

A flow's text beside its canvas in the web editor: type it and the canvas follows, move the
canvas and the text follows, with the same mistakes, hover, completion and format as `holon lsp`.
Written 2026-10-24, before the code; built the same day (`editor/src/TextPane.tsx`,
`useTextPane.ts`, `textSync.ts`). Where the build differs from the plan, the decision says so.

## Where it stands

The editor reads and writes text today, but only at the edges: the flows page imports a `.holon`
(`compileHolon`, a new unsaved tab laid out by the builder's `next_spot`) and exports one
(`printHolon`); the Inspector shows the line a node is written on (`sourceLineOf`). Nothing edits
the text in place. `editing.rs` (`mistakes`, `hover`, `complete`, `format`) answers an editor
from the text alone and was written to be reached through the wasm bridge.

## What it is

A **text** toggle in the flow toolbar opens a pane between the canvas and the Inspector,
resizable by its left edge (the width remembered per browser), open or closed per tab. It shows
the flow as `holon fmt` writes it, or as it was typed. In view mode the same pane shows the
deployed version's text, read only.

## Decisions

1. **CodeMirror 6, not Monaco.** Monaco is several megabytes and a worker setup for features the
   language server already answers; CodeMirror's packages (`@codemirror/view`, `state`,
   `language`, `lint`, `autocomplete`) are small and each takes a function: the lint source asks
   `mistakes`, the hover tooltip asks `hover`, the completion source asks `complete`, the format
   command asks `format`. Highlighting is the language's too: as built, not the lexer (it stops
   at a string not yet closed and drops comments, so half a line would lose its colours) but
   `editing.rs`'s `highlight`, a colourer that reads line by line, never stops, and takes its
   words from the same tables as hover and completion (the TextMate grammar stays VS Code's).
2. **The bridge speaks editing.rs, positions converted at the edge.** holon-wasm gains
   `holon_mistakes`, `holon_hover`, `holon_complete`, `holon_format` and `holon_highlight`, each
   a thin wrapper. The language counts columns in characters, CodeMirror in UTF-16 offsets; the
   conversion is done once, in the bridge (`Offsets`), and `compile_holon` also says the line
   each node is written on.
3. **The text drives the canvas when it compiles.** Each edit is compiled after a pause in typing
   (about 300 ms; a flow compiles in milliseconds, so the pause is for the person, not the
   machine). Compiled, it replaces the canvas's document as one undo step. Not compiled, the
   mistake shows in the text and the canvas keeps the last version that compiled, with a line
   above it saying so; nothing on the canvas is ever half a statement. A text whose structure
   didn't change (a comment, spacing) leaves the canvas alone. The settings the text says
   (`port`, `every`, `browsers`, `optional`) become the flow's; the limits it can't say stay.
4. **Positions follow names.** A compile gives fresh ids and builder positions, so the pane
   carries positions over by each node's key in the text (`textSync.ts`): its name (a binding, a
   `node` statement), a route's request by its route, an inline node by where it is written (the
   input of the node it feeds), a node that feeds nothing by what it is and what it reads. A node
   keeps its place and id while its key is the same and it is the same kind; another kind written
   in its place takes the place under an id of its own; a new node goes beside the first node it
   feeds. The canvas the text was first printed from is matched to that text by the graph itself
   (the two are one graph: colour refinement over what each node is and what it is wired to).
5. **Selection is shared.** A node selected on the canvas marks its line; the cursor on a line
   selects the nodes written on it (`Lowered.spans`, through the match of decision 4).
6. **Format on demand, not on every edit.** The pane keeps what the person typed until they
   format (the button, or Shift+Alt+F as in VS Code): reprinting while typing would move the
   cursor. Building this found `holon fmt` dropping every comment, against §4 of
   `docs/LANGUAGE.md`; it now keeps them (`commands::with_comments`: each comment above the
   statement it was above, found again by what it says whatever its spacing), so Format here, in
   VS Code and on the command line keeps a text's comments.

7. **The last edit wins; structure is identity, shape is only visual** (decided 2026-10-24).
   A canvas edit and the same edit typed in the text are one change to the flow: both compile to
   the same unit, and the `.holon` file says the same thing either way. So both are editors at
   all times, and whichever was edited last is what the other shows. Moving a node changes
   nothing in the text, since a position is not part of the program. The flow keeps the text as
   typed (saved with the document as `source`, used while it still compiles to the same unit),
   so comments survive saving, reopening, text edits and Format. A canvas edit that changes
   structure prints the text again from the canvas, which has no comments: when that drops
   comments, the pane says how many, and undoing the canvas edit brings that text back.

8. **The layout lives in the editor's saved flow only, for now** (decided 2026-10-24; F7 in
   `docs/LANGUAGE.md` §13). Decision 4 keeps positions in the saved document, keyed by names in
   the text. A `.holon` file outside the editor (exported, in git, written by an agent) carries
   no layout and is laid out by the builder on import, as it is today. This is how the tools that
   keep text as the source do it (Mermaid, Graphviz and PlantUML compute layout every time) and
   it keeps the file structure only. A sidecar `.holon.layout` (names to positions, the same
   match as decision 4) is the next step if someone asks to keep an arrangement outside the
   editor; that is the pattern of Structurizr and of Eclipse Sirius and GLSP. Refused:
   positions in the text (a `# at 120,40` comment per statement, the Node-RED and Pure Data
   way), since a program would carry its picture, every diff would show drags, and agents would
   write coordinates.

## What it does not do

No second text for one flow (the pane is the flow's text, not a scratch buffer); no editing a
holon's inner text from inside its composite overlay (the holon is edited where it is written, in
the flow's text); no collaboration (two people editing one flow is a server question, not this
pane's).

## Tests

`editing.rs`: the colours of a text, a half-typed one, and every sample's in order. holon-wasm:
the UTF-16 conversion on a line with an emoji and `→`. `commands.rs`: `fmt` keeps comments
(above, after code, at the end, one inside a string untouched) and is stable. `textSync.test.ts`
(the real core): every sample's canvas matches its text compiled back node for node; keys are
unique; an edit that touches no node keeps every id and place; a changed line keeps the others
and places a new node beside what it feeds; a deleted line removes its node and a renamed
binding is a new one. End to end (`editor/e2e/text.e2e.ts`): open a sample and its text, type a
mistake and see it marked while the canvas stays, fix it and see the canvas follow with the
request where it was, select a node and see its line, complete after `req.`, Format, save and
find the text as typed in the saved flow, delete a node and see the dropped comment said, undo
and see it back.
