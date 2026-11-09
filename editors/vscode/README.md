# Holon for VS Code

Holon flows in VS Code, as `holon check` and `holon fmt` read them. A flow is a TypeScript file,
`NAME.holon.ts`, so VS Code highlights it as TypeScript; for those files the extension adds:

- each mistake as you type, where `holon check` says it is, in its words; a mistake about the
  whole flow (two routes that clash, nothing to start it) at its `flow(…)` line;
- hover: what an imported name, a member, the vocabulary or a leaf is (its signature and what it
  does), a function of the file, a route's request, a binding as the node it is; on `flow`, its
  routes and what it may do;
- completion: what may be written where the cursor is (the file's bindings, functions and
  imported names, the vocabulary, the leaves with their signatures), and after a `.` the members
  (`posts.` → get, put, delete, list; `req.` → params, query, headers, body, json);
- Format Document: the canonical form, as `holon fmt` writes it.

VS Code's own TypeScript reads the file too. For it to know `"holon"` and the import sources
(`secret:`, `host:`, `collection:`, …), the folder needs `holon.d.ts` and a `tsconfig.json`:
`holon new` writes both beside a new flow, and `holon types` writes them into a folder of flows.
Without them every import is "Cannot find module". TypeScript calls the right side of
`{ total } ?? status(400)` unreachable, where Holon reaches it (the language reference, Files).

It starts `holon lsp`, so `holon` must be installed (see the repository's README), on the PATH or
named by the `holon.path` setting. The language server is any editor's: one with a language client
starts `holon lsp` on stdin and stdout. Design: `docs/L4s/language-server-L4.md`.

## The console

`Holon: Open console` (the command palette) shows the Holon console beside the code: the editor
that `holon serve` hosts, with the flows, the canvas, the deployed flows, their runs and history,
and the admin page, all from the server it shows. The panel is the server's own page in a frame,
so it is the same as the browser's; it keeps its state while hidden.

The console is at `holon.consoleUrl` (default `http://127.0.0.1:7420`, `holon serve`'s default). If
nothing answers there and the address is on this machine, the command starts `holon serve` at it,
its output in the Holon output channel. A new server prints the setup code for its first admin
there; the console asks for it. A server the extension started stops when VS Code closes; one
already running is left alone. A console elsewhere (`holon.consoleUrl` on another machine) is only
shown when something answers there.

## Installing

Each release from v0.1.0-rc.10 on carries it as `holon-vscode.vsix`, beside `holon`
(`https://github.com/bekirmfr/holon/releases/download/<tag>/holon-vscode.vsix`):

```sh
gh release download v0.1.0-rc.10 --repo bekirmfr/holon --pattern holon-vscode.vsix
code --install-extension holon-vscode.vsix
```

Or from this folder: `npm install`, `npm run package`, then the same `code --install-extension`.

## Developing

```sh
npm run bundle         # extension.js and its client into dist/extension.js (esbuild)
npm run test:vscode    # bundled, in a real VS Code, against target/debug's holon (cargo build -p holon)
```

`npm run package` bundles first; the `.vsix` holds only the bundle, the manifest and this README.

`test:vscode` downloads a VS Code into `.vscode-test` the first time; `VSCODE_PATH` names an
installed one instead, and `HOLON_PATH` another `holon`.
