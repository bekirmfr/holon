# The web editor

The editor is built into the server: `holon serve` serves it at its own address
(http://127.0.0.1:7420 by default). This guide walks through its parts. For a first run, follow
[Getting started](../getting-started.md) instead.

## Signing in

A new server has no accounts. The first screen, **set up**, asks for the setup code the server
printed when it started, then your name and a password: that makes you its admin. After that,
everyone signs in with a name and password, or through an identity provider if the admin set one
up. An admin adds people from the **admin** page, and each gets a one-time link to set their
password.

Your name is at the right of the header. Its menu has **welcome and docs**, **change password**
and **sign out**.

## Pages and tabs

The header has **new** (a new flow), **flows**, **deployed**, **docs** and, for admins, **admin**.
Each page opens in a tab, before the flow tabs; every tab closes with its ×. **new** opens an
untitled canvas at once; the flow is named when you first save or deploy it. A name another saved
flow or a deployment already has asks before that one is replaced.

- **home** is shown when no flow is open. **customize** adds, moves and removes its panes:
  recent flows, deployments, activity, samples, notes and a getting-started checklist. Your panes
  are kept with your account.
- **welcome** has the steps to start and short docs. Untick **show this on start** to stop it
  opening.
- **docs** is this manual, its contents beside the page. Its links stay in the tab, and each
  flow in it has **open in a tab**, which opens the flow as an unsaved copy to try.

A flow tab shows the flow's name, **•** when it has unsaved changes, and whether it is a
**draft**, **saved** or **deployed**. Closing a tab with changes asks first.

## The flows page

- **saved flows**: each with who saved it and when, and **open**, **publish**, **.holon**
  (download) and **×** (delete). **manifest ▸** lists what the flow may do. A flow an agent saved
  says so: "saved … by *name* (agent)".
- **import .holon** opens a text file as a new, unsaved tab.
- **marketplace**: where flows are published to and placed from (see
  [The marketplace](#the-marketplace)).
- **samples**: one flow for each thing Holon can do. **open** makes an unsaved copy in its own
  tab, to change, save and deploy. A sample that needs secrets says which.

## The canvas

A flow tab is a canvas of nodes and wires.

- **Placing.** The palette on the left has the saved **flows** (each placed as one node), the
  **marketplace**, the **http** templates (a route's request and respond, Stripe's verify), and
  the **palette** of every leaf. Drag one onto the canvas. Each entry's **ⓘ** says what it does.
- **Wiring.** Drag from an output to an input. The types must fit, and an input takes one wire.
  Wires are coloured by type.
- **Parameters** are set on the node itself (a method, a key, a constant). A node that takes any
  number of inputs has **+** and **−**.
- **Selecting.** Click a node or wire; shift-click or drag a box for several. **Delete** or
  **Backspace** removes the selection.
- **Undo** is **Ctrl+Z** (**⟲**), redo **Ctrl+Y** or **Ctrl+Shift+Z** (**⟳**), per tab.

### Holons on the canvas

Select two or more nodes and press **group** (bottom right) to make them one holon node.
**ungroup** puts its nodes back. A holon's **expand** opens it in place: change it like a flow,
then **apply**, and it becomes a new version in the flow (undo takes it back). **Esc** goes back a
level. Its **inputs** and **outputs** boxes add, rename and remove ports.

A saved flow placed as a node shows **↻ update** when that flow has been saved since. Updating
asks first if the new version changes ports that are wired.

### The inspector

The panel on the right shows the selected node: what it does, its kind, its line in the text,
its parameters, inputs and outputs. **Unit** shows what the node's unit is made of. **›** folds
the panel.

### The server panel

The **server** panel at the bottom left lists the flow's routes. **test** opens a request
tester: pick a method, a path (`/todos/1?full=1`), headers and a body, then **send local** or
**send live**. Local runs the flow as it is on the canvas, in your browser, with nothing from
outside: no stored state, no secrets, no calls, so what needs them has no value. Live sends the
request to the deployment, as a real run that is logged.

## The text pane

**text on** in the toolbar opens the flow's text beside the canvas (it opens by itself on a wide
window). It is the same flow:

- typing changes the canvas after a short pause; a mistake is underlined and explained, and the
  canvas keeps the last version that compiled;
- a change on the canvas writes the text again;
- the cursor in the text selects the node under it, and a node selected on the canvas marks its
  text;
- hover shows what a name is, and completion offers what may follow (after `req.`, say);
- **Format** (**Shift+Alt+F**) writes the text as `holon fmt` would.

Saving keeps the text as you typed it, comments included. A canvas change that rewrites the text
can drop a comment; the editor says so, and undo brings it back.

## Saving and deploying

**save** keeps the flow on the server under its name. The first save asks for one: letters,
digits, `-` and `_`.

The deploy bar (edit mode, in the toolbar) has **deploy** (or **update**, with **•** when the
canvas differs from what is deployed), the deployment's state and port, **pause**, **resume**,
**stop** and **run**, and **⚙** for its settings: port, interval, limits, secrets and optional
permissions. Deploying shows what the flow may do and asks you to approve what isn't granted yet.
[Deploying and permissions](deploying.md) covers it in full.

## View mode: replaying runs

Once a flow is deployed, the toolbar's **edit | view** switch opens **view** mode: the deployed
flow, read only, with its runs. (A deployment's **view** button on the deployed page opens it
too.)

The **run** pane at the bottom has two tabs:

- **Log**: the runs, newest last, with their route, status and time. **● live** follows the
  newest; picking one turns it off. A run's details show its request, what it read, what it did,
  its answer and its outbound calls. Above the list, the chain bar says **chain unbroken** (or
  where it breaks), the log's **head**, and **export runs**
  ([Checking runs](checking-runs.md)).
- **Timeline**: the nodes in the order they ran, or slowest first, with how long each took. **✎**
  marks state writes.

On the canvas, each node that ran shows its value beside its outputs (click one to pin the whole
value; **values off** hides them). What didn't run is dimmed: a node on a branch not taken has a
dotted border, and the inspector says why it didn't run. **◀** and **▶** step through the run
one node at a time. The text pane does the same: what didn't run is dimmed, and values sit beside
expressions.

The header of the run pane says **✓ replay reproduces the run** when the editor, running the flow
again from the log, got the same answer. A run on an earlier version is shown on the canvas it
ran on.

**A failed run** opens on the node where it failed, outlined in red, with the reason. Nodes that
didn't get their input because of it are marked **↯**. **show** in the banner picks the failing
node.

**Flow state** (a tab of the inspector, in view mode) shows each state key and collection before
and after the run, or after the step you are on.

## The deployed page

Every deployment, as a card: its state, port, triggers and who deployed it, with **pause**,
**resume**, **stop**, **run** (from the latest state, fresh, or a kept session), **view**, and
**delete** once stopped. Its folds:

- **sessions ▸**: each session's runs, errors and state; **restore** starts from one.
- **⏸ N waiting ▸**: runs parked at an `await`.
- **manifest ▸**: what it may do, and what isn't granted.
- **permissions ▸**: what is granted, with **revoke** and **grant back**, limits that were
  lowered, and the latest changes ([Changing a grant](deploying.md#changing-a-grant)).

The **activity** list at the bottom says who did what, and when.

## The admin page

Admins manage, from here:

- **people**: add someone with a role (they get a link to set their password, good once, for
  seven days), change a role, reset a password (their sessions end and the old password stops
  working at once), disable, or remove;
- **API tokens**, for scripts, CI and agents on other machines: each with a role, shown once;
- **folders**: the named folders flows may use ([Files on the host](files.md));
- **signing in through an identity provider** (OpenID Connect): its issuer and client id.

## The marketplace

A marketplace is itself a flow (the **Marketplace** sample) that holds published holons. Set its
address on the flows page (**set**). Then **publish** on a saved flow publishes it, and its
entries appear in the palette to place. Placing one shows what it may do first. A publisher can
**yank** an entry: it stays listed, flagged, and flows that placed it keep it.

## Other editors

The text is a plain `.holon` file, and `holon lsp` is a language server for any editor that can
start one: mistakes as you type, hover, completion and formatting. The VS Code extension
(`holon-vscode.vsix`, on each release) highlights the language and starts `holon lsp`.

## Shortcuts

| Keys | Does |
|---|---|
| Ctrl+Z | undo |
| Ctrl+Y, Ctrl+Shift+Z | redo |
| Delete, Backspace | remove the selection |
| Shift+Alt+F | format the text |
| Esc | close a dialog, the settings, or a level of an expanded holon |
| Enter | confirm a dialog |
| middle click | close a tab |
