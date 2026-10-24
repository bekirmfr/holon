# Holon for VS Code

`.holon` flows in VS Code, as `holon check` and `holon fmt` read them:

- highlighting: headers, routes, holons, bindings, calls, the nouns for the outside world
  (`secret`, `state`, `http`, …), strings with their `{…}` parts;
- each mistake as you type, where `holon check` says it is, in its words; a mistake about the
  whole flow (two routes that clash, nothing to drive it) at its `flow` header;
- hover: what a keyword, noun or leaf is (its signature and what it does), a holon of the file, a
  collection, a route's request, a binding as the node it is; on the flow's name, its routes and
  what it may do;
- Format Document: the canonical form, as `holon fmt` writes it.

It starts `holon lsp`, so `holon` must be installed (see the repository's README), on the PATH or
named by the `holon.path` setting. The language server is any editor's: one with a language client
starts `holon lsp` on stdin and stdout. Design: `docs/L4s/language-server-L4.md`.

## Installing

From this folder:

```sh
npm install
npm run package
code --install-extension holon-0.1.0.vsix
```

## Developing

```sh
npm test               # the grammar, through VS Code's TextMate engine, on every sample
npm run bundle         # extension.js and its client into dist/extension.js (esbuild)
npm run test:vscode    # bundled, in a real VS Code, against target/debug's holon (cargo build -p holon)
```

`npm run package` bundles first; the `.vsix` holds only the bundle, the manifest, the language
configuration, the grammar and this README.

`test:vscode` downloads a VS Code into `.vscode-test` the first time; `VSCODE_PATH` names an
installed one instead, and `HOLON_PATH` another `holon`.
