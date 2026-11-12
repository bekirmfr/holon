# The web editor

The editor is built into the server: `holon serve` serves it at its own address
(http://127.0.0.1:7420 by default). This guide walks through its parts. For a first run, follow
[Getting started](../getting-started.md) instead.

## Signing in

A new server has no accounts. The first screen, **set up**, asks for the setup code the server
printed when it started, then your name and a password: that makes you its admin. After that,
everyone signs in with a name and password, or through an identity provider if the admin set one
up. An admin adds people from **People and access**, and each gets a one-time link to set their
password.

Your initials are at the right of the top bar. Their menu has **Welcome and docs**, **Change
password** and **Sign out**; the theme (**Auto**, **Light** or **Dark**) is beside them.

## Pages and tabs

The nav on the left has **new flow**, then the pages, each with its key: **Overview** (1),
**Approvals** (2, with how many things wait), **Flows** (3), **Editor** (4) and **Library** (5);
for admins, the host's pages, **Resources** (6), **People and access** (7) and **Policy** (8), and
for anyone else **Your agents and services** (7); and **Getting started** and **Docs**. Under them, the host card says which server this is, whether its
run log is intact, how many runs it holds and the newest head, as of the time it shows (read every
minute; its refresh button reads it now). The version the server runs is beside the name, top left. The top bar
says where you are (each step back a link), and has the search (**Ctrl K** or **/**: a page, a
flow or a command), the interface, the theme and your account.
Every place has its own address (`#overview`, `#pulse.history`, `#approvals.shout.v1`), so the
browser's back and forward work, and a link opens where it says.

**new flow** opens an untitled canvas in the editor at once; the flow is named when you first save
or deploy it. A name another saved flow or a deployment already has asks before that one is
replaced. The editor keeps each flow open in a tab of its own, after its **Home** tab; every tab
closes with its ×.

- **Overview** is where the console opens: what needs a person, four tiles (runs in the last 24
  hours, the share that failed, the flows live, the run log), every deployed flow with its runs
  hour by hour (red where one failed), the newest runs across every flow, and the run log, which
  **Check now** reads again line by line. It is read from the server's `GET /summary`.
- **Approvals** is what waits for a person: **Deploys**, the versions to review and deploy (an
  agent's proposal with its why and note, and **Decline with a reason…**), and **Runs waiting**.
  **Review** opens one: what it may do, its changes, *If approved*, *On recent runs* (a deployed
  flow's new version replayed on its last runs, [Replayed on recent runs](deploying.md#replayed-on-recent-runs)),
  and Approve, keep it saved, or decline it. **Runs waiting** lists the runs waiting at an
  `approval`, each step and who has taken it so far: **Approve**, or **Decline…** with a reason,
  on the steps you may take (*Approve as reviewer…* when there are several). Each principal takes
  one step, once, never of a run they started, and the host's [approval
  settings](people.md#who-decides-an-approval) can keep out whoever deployed the version, and
  agents.
- **Getting started** has the steps to start, each ticked once done, your notes (kept with your
  account) and short docs. Untick **show this on start** to stop it opening.
- **docs** is this manual, its contents beside the page. Its links stay in the tab, and each
  flow in it has **open in a tab**, which opens the flow as an unsaved copy to try.

The top bar's **Clean | Terminal** (or **t**, anywhere) switches the interface. Terminal is one
dark look, mono and square, with `[ ]` buttons, each pane's key in its header, and a status bar
under the page: the flows and how many are live, what waits for a person, the run log's newest
head and whether it is intact, and the keys worth knowing where you are. The choice is kept with
your account, as the theme is.

On a narrow screen the nav is a rail of icons; under 880 px a page's side columns go under it; on
a phone the top bar's **☰** opens the nav as a drawer, and the editor shows one pane at a time,
the canvas, the text or the Inspector, as the switcher in its bar says.

A flow tab shows the flow's name, **•** when it has unsaved changes, and whether it is a
**draft**, **saved** or **deployed**. Closing a tab with changes asks first.

## Flows and the Library

**Flows** is every flow in one table, saved and deployed alike: its state (a **draft** not
deployed yet; live, paused or stopped; runs waiting), what it may do, its runs and when it last
changed, with **All**, **Live**, **Waiting** and **Paused** to narrow it and a search. A deployed
flow opens its own page ([A flow's page](#a-flows-page)); a draft opens in the editor. A saved
flow's row also has publish, download (`.holon.ts`) and delete. A flow an agent saved says so, by
the agent's name: "saved … by *bma-agent*". **Import** opens a flow's text, `NAME.holon.ts`, as a new, unsaved
tab.

The **Library** has three tabs:

- **Tutorials**: the manual's walkthroughs.
- **Samples**: one flow for each thing Holon can do, with a search and what each may do. **open**
  makes an unsaved copy in its own tab, to change, save and deploy. A sample that needs secrets
  says which.
- **Marketplace**: where flows are published to and placed from (see
  [The marketplace](#the-marketplace)).

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
- **Secrets.** Every node a secret's value reaches where it leaves the computation is marked 🔑,
  with what it does with it: a request it is sent in ("sends the secret "slack_webhook_path" to
  https://hooks.slack.com"), a database read it is a value of, a write or a call the host
  refuses, the route's answer; and where what a call answers to it goes ("sends what is answered
  to the secret "client_secret" to https://httpbin.dev"). A placed holon is marked for what its
  insides do. A node that only computes with it (an HMAC, Stripe's verify) isn't.

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

### The tools panel

The **tools** panel on the right lists what starts the flow's runs: its clocks (`every 2s`),
its file handlers (`on file inbox`), its actions and its routes, with a count when it is folded.
**tick now** by a clock runs it once, now, on the deployment. Each action has a box for what to
send it, as JSON, and **run**: the answer shows under it with the run's number, and **view run**
opens that run. A viewer sees the actions and can't run them
([Actions](actions.md)). When the canvas has changes not deployed, a run uses the deployed version,
and the panel says so.

**test** opens a request
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
**stop** and **run**, and **⚙** for its settings: port, limits, secrets, optional
permissions and which actions agents may run. Deploying shows what the flow may do and asks you
to approve what isn't granted yet. [Deploying and permissions](deploying.md) covers it in full.

A flow with no route, clock, file handler or action starts nothing, so **deploy** is greyed and
says why: add one. An action is the one a person runs by hand.

## View mode: replaying runs

Once a flow is deployed, the toolbar's **edit | view** switch opens **view** mode: the deployed
flow, read only, with its runs. (A deployment's **view** button on the deployed page opens it
too.)

The **run** pane at the bottom has two tabs:

- **Log**: the runs, newest last, with what started each (its route, or `every 2s`, `on file
  inbox`, `action reset`), its status and time. **● live** follows the
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

## A flow's page

A deployed flow has a page of its own (`#pulse`): its name and state, **Pause**, **Stop** or
**Resume**, **Open in editor**, and six tabs, each with its own address (`#pulse.history`):

- **Status**: its last 24 hours, hour by hour, and its timings; where it runs (its port and
  triggers: `Http :8156`, `every 2s`, `on file inbox`, `action reset`); what it may do, since
  which version and approved by whom; its latest runs; its versions; and its actions, each with
  its box and **run** ([Actions](actions.md)).
- **Runs**: its runs, newest first; one opens on a page of its own, step by step.
- **Logs**: what its flow wrote with `console.log` and the rest, as they come, in a terminal-style
  pane: a time, a level, the run that wrote it (a link to the run), and the text. A filter shows
  warnings or errors only; **Following** keeps reading, and pauses. A run's page shows the lines up
  to the end of that run, and **only run #N's** narrows them to its own. They are kept beside the
  runs, not in them; `holon logs` reads the same ([`holon logs`](../reference/cli.md#holon-logs)).
- **History**: what happened to it, runs and what was done to it (deploys, access, lifecycle),
  with a filter. A row says what happened, and what going back to it would do; **Go back**
  restores an earlier version or state ([Going back](deploying.md#going-back-to-an-earlier-version)).
- **State**: its live state, cell by cell.
- **Settings**: what is granted, with **revoke** and **grant back** and limits to lower
  ([Changing a grant](deploying.md#changing-a-grant)); its secrets; the settings it was deployed
  with; **agents may propose versions**; its waiting runs; its sessions (**restore** starts from
  one); what it holds, in memory and on disk
  ([What it holds](../reference/server.md#what-it-holds)); and removing it, once stopped.

## The host's pages

An admin's, in the nav's second group:

- **Resources**, in tabs: **Databases** and **Folders** flows use by name
  ([Files on the host](files.md), [Databases](databases.md)); **Mail** and **Feeds**; the
  server's own **Secrets**, read by every deployment that reads one and has none of its own, each
  with where it may go ([The server's own secrets](deploying.md#the-servers-own-secrets)); **Host
  and storage**: what the host warns of ([Warnings](../reference/server.md#warnings)), when it
  warns of a full disk (`85%`, or `5G` free) and how it acts on one (only warn, pause what records
  runs, or cut the oldest runs), where the runs logs let go of are backed up, what the server and
  each flow hold, as `holon host` says, how often it sweeps (`[storage] sweep_every`) and sweeping
  now, as `holon store sweep` and `holon folder sweep` do; and **Who may touch what**, each
  resource with the deployments granted it.
- **People and access**: people (add someone with a role, and they get a link to set their
  password, good once, for seven days; change a role, reset a password, disable, or remove);
  **agents and services**, each with a role and an owner, and their tokens, each shown once
  (for agents on other machines, scripts and CI); and signing in through an identity provider
  (OpenID Connect). Someone who isn't an admin has **Your agents and services** instead: the
  agents and services they own, whose tokens they make (each shown once) and revoke; an admin adds
  one and names its owner.
- **Policy**: the host's `holon.toml`: what it grants without asking, which exceptions it ever
  grants, whether agents may propose versions, and each limit's default and ceiling.

The **Overview**'s *What was done* says who did what, and when. Every one of these has a command
too: `holon people`, `holon oidc`, `holon folder`,
`holon database`, `holon secret bind`, `holon store sweep`, `holon grants` and `holon policy`
([Command line](../reference/cli.md)).

## The marketplace

A marketplace is itself a flow (the **Marketplace** sample) that holds published holons. Set its
address on the flows page (**set**). Then **publish** on a saved flow publishes it, and its
entries appear in the palette to place. Placing one shows what it may do first. A publisher can
**yank** an entry: it stays listed, flagged, and flows that placed it keep it.

## Other editors

The text is a TypeScript file, `NAME.holon.ts`, so any editor highlights it, and `holon lsp` is a
language server for any editor that can start one: mistakes as you type, hover, completion and
formatting. The VS Code extension (`holon-vscode.vsix`, on each release) starts `holon lsp` for
`.holon.ts` files.

## Keys

**?** shows every key, anywhere. The ones to know:

| Keys | Does |
|---|---|
| 1 … 8 | go to a page: overview, approvals, flows, editor, library, resources, people, policy |
| Ctrl+K, / | find a page, a flow or an action; `:` for a command |
| Esc | go up (a run to its runs, a flow to the flows); close a dialog or a level of a holon |
| F6, Shift+F6 | the next or previous pane |
| t | switch the interface: Clean or Terminal |
| Ctrl+S, Ctrl+D | save the flow, deploy it (in the editor) |
| e | edit the flow, or view its runs |
| c, x, i | show or hide the canvas, the text, the Inspector |
| j k, ← → | step through a run, a node at a time (viewing runs) |
| , . | the run before, or after |
| [ ] | the version waiting before, or after (a review) |
| y, Enter / n, Esc | a dialog's yes, or no |
| Ctrl+Z / Ctrl+Y, Ctrl+Shift+Z | undo / redo |
| Delete, Backspace | remove the selection |
| Shift+Alt+F | format the text |
| middle click | close a tab |
