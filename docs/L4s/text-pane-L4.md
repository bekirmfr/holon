# The web editor's text pane (L4)

A flow's text beside its canvas in the web editor: type it and the canvas follows, move the
canvas and the text follows, with the same mistakes, hover, completion and format as `holon lsp`.
Written 2026-10-24, before the code.

## Where it stands

The editor reads and writes text today, but only at the edges: the flows page imports a `.holon`
(`compileHolon`, a new unsaved tab laid out by the builder's `next_spot`) and exports one
(`printHolon`); the Inspector shows the line a node is written on (`sourceLineOf`). Nothing edits
the text in place. `editing.rs` (`mistakes`, `hover`, `complete`, `format`) answers an editor
from the text alone and was written to be reached through the wasm bridge.

## What it is

A **Text** toggle in the flow toolbar opens a pane to the right of the canvas, in edit mode,
resizable, remembered per tab. It shows the flow as `holon fmt` writes it. In view mode the same
pane shows the deployed version's text, read-only.

## Decisions

1. **CodeMirror 6, not Monaco.** Monaco is several megabytes and a worker setup for features the
   language server already answers; CodeMirror's packages (`@codemirror/view`, `state`,
   `language`, `lint`, `autocomplete`) are small and each takes a function: the lint source asks
   `mistakes`, the hover tooltip asks `hover`, the completion source asks `complete`, the format
   command asks `format`. Highlighting uses the same lexer through a `tokens(source)` bridge
   call, so the pane and `holon check` can't disagree about what a word is (the TextMate grammar
   stays VS Code's).
2. **The bridge speaks editing.rs, positions converted at the edge.** holon-wasm gains
   `holon_mistakes`, `holon_hover`, `holon_complete`, `holon_format` and `holon_tokens`, each a
   thin wrapper. The language counts columns in characters, CodeMirror in UTF-16 offsets; the
   conversion is the language server's, done once in `holonText.ts`.
3. **The text drives the canvas when it compiles.** Each edit is compiled after a pause in typing
   (about 300 ms; a flow compiles in milliseconds, so the pause is for the person, not the
   machine). Compiled, it replaces the canvas's document as one undo step. Not compiled, the
   mistake shows in the text and the canvas keeps the last version that compiled, with a line
   above it saying so; nothing on the canvas is ever half a statement.
4. **Positions follow names.** A compile gives fresh ids and builder positions, so the pane
   carries positions over by name: a node keeps its place while its name in the text (a binding,
   a `node` statement, a route's request) is the same, and an inline node keeps it while its
   consumer and port are; a new node goes beside its first consumer. Ids follow the same match,
   so selection, the Inspector and undo survive an edit.
5. **Selection is shared.** A node selected on the canvas highlights its line (`printHolonLines`
   already says which); the cursor in a line selects the node it lowers to (`Lowered.spans`).
6. **Format on demand, not on every edit.** The pane keeps what the person typed until they
   format (the toolbar button, or Shift+Alt+F as in VS Code): reprinting while typing would move
   the cursor.

7. **The last edit wins; structure is identity, shape is only visual** (decided 2026-10-24).
   A canvas edit and the same edit typed in the text are one change to the flow: both compile to
   the same unit, and the `.holon` file says the same thing either way. So both are editors at
   all times, and whichever was edited last is what the other shows. Moving a node changes
   nothing in the text, since a position is not part of the program. The flow keeps the text as
   typed (saved with the document as `source`, used while it still compiles to the same unit),
   so comments survive saving, reopening and text edits. A canvas edit that changes structure
   reprints the text, and the printer writes the doc comment but not comments inside the flow:
   when that drops comments, the pane says how many, with undo to get them back.

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

The bridge: each wrapper against `editing.rs`'s answers on a sample, and the UTF-16 conversion on
a line with `→` and an emoji (vitest). The match: an edit that renames nothing keeps every
position and id, a renamed binding gets a new place, a deleted line removes its node, an inline
node keeps its place under its consumer. End to end (`editor/e2e`): open a sample, open the pane,
type a mistake and see it marked while the canvas stays, fix it and see the canvas change, select
a node and see its line, complete after `posts.`, format, save and reopen with the text as typed.
