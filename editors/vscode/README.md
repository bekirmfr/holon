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

## The sidebar

The Holon icon in the activity bar opens the server's deployments and saved flows as VS Code's own
views, read through the `holon` command with `--server` and `--json`:

- **Deployments**: each deployment, its status and errors. A click on one expands its overview: a Status group (how it runs, its errors, who deployed it, and its Manifest and History), then the groups below. Its buttons run, pause or stop it, open its runs and its history.
  Expanding a deployment shows three groups: **Needs approval** (grants its version asks for that
  nothing has approved; the check button approves one, `holon grant --grant`), **Waiting for an
  event** (runs parked for a flow's own link, read-only), and **Recent runs** (the last ten; a click
  opens its record as JSON, from `holon runs --json`).
- **Flows**: each saved flow, with three icon buttons: **edit** (its text, as TypeScript), **graph**
  (its nodes with their ports, types and params, and the wires between them, in the editor's colours). A click selects a node, a wire or a port. Dragging a node moves the selected nodes, and dragging on the background selects the nodes in a box. Ctrl or Shift adds to the selection, and Escape clears it. The middle button drags the picture, the wheel zooms, and double-click (or the fit button) fits it to the window. The graph opens tidied: each node sits in the column of its depth, so every wire runs left to right, and the tidy button (▤) lays it out again after you have moved nodes. Each handler's nodes form a domain: they stay together in a frame labelled with the handler's method and path, apart from the other handlers' domains. Nodes wired to two handlers join them into one domain and **deploy** (`holon deploy`, as the editor's Deploy does). Edit
  saves back to the server: `Ctrl+S` writes the flow with `holon flows save`, and a flow someone else
  saved asks before it is replaced. A deploy that needs approval shows what it asks for, and
  approves it on a yes.

A deployment's **State** group lists its values, and a click on one opens its history (`holon history --cell`)
as a read-only editor. Its collections open their items as JSON (`holon state --collection`).

- **Activity**: what has happened on the server, newest first (`holon activity`).

**New flow** (the + on the Flows view, or `Holon: New flow` in the palette) asks for a name and writes a starter flow with `holon new`, beside the TypeScript it needs. It opens the file, and offers to save it to the server so it shows in Flows.

The Flows view also lists, under **Not on the server**, the `.holon.ts` files of the workspace that the server does not keep yet. **Save flow to server** (the cloud button on an open flow file, on a flow in the explorer, or on an item under Not on the server) saves a flow file to the server, where it shows in Flows.

Each deployment has pause (or resume, when it is paused or stopped), stop, runs and history icon buttons, set by its status. Runs opens the Holon Runs panel at the bottom of the window, with the deployment's last 200 runs: the runs on the left, newest first, and the selected run on the right, in four tabs as the web and terminal consoles have them: **Detail** (its record, its values as an expandable list), **Steps** (the flow's text, with each statement's value under it), **Timeline** (when each step ran, to scale) and **Error** (what a failed run stopped at). A checkbox shows only the failed runs, and Open as JSON opens the selected run's whole record beside the editor. Each flow has a trash button: a saved flow is deleted from the server, and a flow file of the workspace is moved to the trash. Both ask first.

A flow has explain and manifest buttons. Explain opens what the flow does, as `holon show` outlines it: its routes, and each node with what feeds it. Manifest opens the Holon manifest: what the flow touches, what starts it and what it may do, as `holon deploy --manifest` shows it. Nothing is deployed. A click on a flow still opens its text.

The views refresh every 10 seconds and on the refresh button.

The server is `holon.consoleUrl` (default `http://127.0.0.1:7420`, `holon serve`'s default). The
sidebar needs a signed-in server. `Holon: Sign in to the server` asks for a name and password (on a
new server, the setup code `holon serve` prints in the Holon output channel, for the first admin).
The sign-in is kept in VS Code's secret storage for that server, and passed to `holon` as
`HOLON_TOKEN`. `Holon: Start the local server` starts `holon serve` at a loopback `consoleUrl`
when nothing answers there, with its output in the Holon output channel; a server the extension
started stops when VS Code closes, and one already running is left alone.

Not yet in the sidebar: the editor's other pages (state, secrets, folders, admin, deploying from
the sidebar). The browser console at `holon serve` has them.

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
