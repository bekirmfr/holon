# Editors: `holon lsp` and the VS Code extension (L4)

Step 8 of `docs/LANGUAGE.md` §12: a `.holon` file read by an editor as it is typed, the way
`holon check` reads it. Written 2026-10-24, before the code.

## What it is

`holon lsp` is a Language Server Protocol server on stdin and stdout (JSON-RPC 2.0, each message
framed by a `Content-Length` header), started by the editor. It needs no server and no sign-in:
everything it says comes from the text, through the same code as `holon check`, `holon fmt` and
`holon mcp`'s `reference` (holon-author's `editing.rs`, on `lang/`). So a person in an editor, the
command line and an agent read a flow alike.

```
holon lsp
```

`editors/vscode` is the VS Code extension: the language (`.holon`, `#` comments, brackets), a
TextMate grammar for highlighting, and a client that starts `holon lsp` (the `holon.path` setting,
default `holon` on the PATH). Any other editor with a language client uses `holon lsp` as is.

## What it answers

| Request | Answer |
|---|---|
| a text opened or changed | its mistakes, published as diagnostics: the first one it can't compile past, with its span and `holon check`'s words; or, compiled, what is wrong with the flow as a whole (a clash of routes, no driver), at its `flow` header |
| hover | the word under the cursor: a keyword or noun in a line; a leaf or template by its signature and what it does (the reference); a holon of the text by its signature; a collection; a route's request and its parts; a binding as the node it is; the flow's name as what `holon check` says of it: its routes and what it may do |
| format | the text as `holon fmt` writes it, one edit; a text with a mistake is left as it is |

## Decisions

1. **The language is holon-author's; the protocol is `holon`'s.** `editing.rs` holds what an
   editor asks (`mistakes`, `hover`, `format`) as plain functions with tests, so the web editor
   can use them through the wasm bridge later; `crates/holon/src/lsp.rs` only frames messages and
   converts positions.
2. **Whole text, every change.** The client sends the full text on each change
   (`TextDocumentSyncKind.Full`) and the server compiles it again: a flow is small, and a compile
   takes milliseconds. No incremental parsing, no debounce.
3. **One mistake at a time, as `check` says it.** The compiler stops at the first mistake, so
   there is one diagnostic where it stopped. Recovering to report more would make the messages
   differ from `holon check`'s, and the second would often be caused by the first.
4. **Positions in UTF-16, converted at the edge.** The language counts columns in characters;
   LSP counts UTF-16 code units (VS Code offers nothing else). The server converts each position
   against the line's text, so a `→` or an emoji in a string doesn't shift what follows.
5. **Highlighting is the grammar's, not the server's.** A TextMate grammar colours a file before
   the server starts and when it isn't installed; semantic tokens can come later if the grammar's
   guesses (a call is a word before `(`, a binding a word before `=`) prove wrong.
6. **The extension is small and plain JavaScript.** One file, `vscode-languageclient` its only
   dependency, bundled with it by esbuild into `dist/extension.js` (`npm run bundle`), so the
   `.vsix` (`npm run package`, `@vscode/vsce`) holds that, the manifest, the language
   configuration, the grammar and the README: seven files, no `node_modules`.

## What it does not do

No completion, go-to-definition, rename or code actions yet; no workspace-wide state (each file
is a flow on its own); no deploying from the editor (that is the web editor's, where a person
approves).

## Tests

`editing.rs`: every sample has no mistakes; a mistake's span and words; a mistake about the flow
at its header; each kind of hover on the `slugs` sample; format is `fmt`. `lsp.rs`: the protocol
on framed messages, initialize to shutdown, with a change publishing a diagnostic and positions in
UTF-16. The extension: the grammar tokenizes every sample (`vscode-textmate`), and an integration
test in a real VS Code (`@vscode/test-electron`) opens a `.holon`, sees its mistake, hovers and
formats through `holon lsp`.
