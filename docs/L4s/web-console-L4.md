# The web console: one design from the editor and the merged mockup (L4)

**Status:** design, before the code. Written 2026-11-12 against rc.35 (`d289e435`). Every claim
about the code as it is cites a file and line at that commit.

Two designs of the web console exist:

- **The editor**, `editor/src`, which runs.
- **"Holon Console, merged"**, a clickable mockup (a private artifact,
  `claude.ai/artifact/AENBJEuTkeZDZ2geNnNskq`, 2026-11-08). It put three earlier designs together:
  the Console, the Redesign and the Terminal design.

The gap report both were measured against, `gaps-web.md`, is cited by the code's comments
(`G8`, `G16`, `G17`, `G25`, for example `App.tsx:754`, `LibraryPage.tsx:1`) but isn't in the
repository. This note replaces it and the mockup as the console's target.

Each side has what the other lacks:

- **The mockup reads and decides better.** One picture of a flow by its statements; approvals and
  reviews that show what changes, line by line; keys for everything; a palette that finds runs.
- **The editor works.** It has the node canvas, every resource kind, approvals by people (k of n),
  the setup and invite screens, the whole manual and a full run list. In places the mockup is
  behind it: its approvals are `event("approved")` with a resume route, and it marks G11 and G12 as
  open questions.

Builds on:

- `access-and-approvals-L4.md`, whose People pages (§12) and "buttons follow `can`" (step 7) this
  note leaves to it;
- `text-pane-L4.md` and `text-graph-link-L4.md` (the text and its link to the graph);
- `history-L4.md` (the timeline and restore);
- `host-resources-L4.md` (resources across the host).

## 1. Where it stands

**The shell matches the mockup closely:**

- the nav groups and keys 1 to 8, with the host group gated by role (`ui/Shell.tsx:42`, `:49`,
  `:57`);
- the Clean and Terminal looks, with a status bar in Terminal (`Shell.tsx:227`, `:64`);
- the host card (`Shell.tsx:305`);
- the command palette, with `:` commands (`commands.ts:59`; entries `App.tsx:833`–`:848`);
- the keys dialog (`keymap.ts:26`);
- the Approvals tabs;
- the three-column Review page;
- the flow page's tabs (`FlowPage.tsx:21`–`:26`);
- History's restore of code, state or both.

Both write flows as `.holon.ts`.

**The editor's workspace, left to right:**

1. the primitive Palette (`App.tsx:1119`);
2. the may-strip (`:1130`), above the canvas, the text pane (`:1246`) and the Inspector
   (`:1247`; `ViewPanes.tsx:1220`);
3. the Tools panel (`:1411`; `ServerPanel.tsx:131`), which holds the trigger rows and the request
   tester;
4. the Deployment panel (`:1434`; `DeployBar.tsx:592`), which holds port, limits, network,
   permissions and secrets.

The canvas is React Flow over every node. The Review page's Canvas tab draws those nodes as a
static SVG (`ReviewPage.tsx:351`).

**A flow by its statements already exists, in Rust.**

- `holon_author::graph::graph(source, every)` (`crates/holon-author/src/graph.rs:81`) reads the
  `.holon.ts` text into **bands**, one per trigger (`:56`).
- Each band has **rows**, one per statement (`:29`). A row has:
  - its lines (`line`, `last`);
  - its name, and what its outermost nodes are (`does`);
  - whether it acts on the world (`effect`);
  - which earlier rows it takes from (`from`), and what it takes from outside its band
    (`outside`);
  - the nodes it is made of (`parts`, `:16`).
- The terminal editor draws it, coloured by what a statement does (`workbench/graph.rs:19`).
- The web doesn't have it: the wasm bridge exports `holon_mistakes`, `holon_usage` and
  `holon_highlight` (`crates/holon-wasm/src/lib.rs:1247`, `:1268`, `:1313`), but not the graph.

**Access is principals now** (access steps 1 to 3, rc.35):

- `/auth/me` answers `kind` and `owner` (`server.ts:107`);
- `/auth/me/can` answers whether the caller may do each action, and why not (`server.ts:101`).
  Only Approvals asks it (`useApprovals.ts:46`);
- the route table asks `resource.view` for reading resources (`can.rs:488`). The built-in viewer
  and editor hold `policy.view` and none of the host's other views (`can.rs`'s `BUILTIN`); a role an
  admin defines may hold them. But the nav shows the host pages only to an admin, and to a person
  their own agents and services (`Shell.tsx:57`).

**What the server doesn't answer yet**, which the views below need:

- a flow's median run time in `/summary`. `FlowSummary` (`summary.rs:56`) has runs and failures
  by hour; `Latest` has each run's `duration_ms` (`:83`);
- the policy file's text. `GET /permissions` answers the parsed policy (`deploy.rs:819`);
- when a principal last signed in, or a token was last used;
- a setting changed in place. `POST …/settings` changes only `agents_propose`
  (`proposals.rs:298`); limits are lowered through the grant;
- a note kept with a grant;
- one run's export. `GET …/export` exports the whole deployment (`verify.rs:95`).

## 2. Goals and non-goals

**Goals:**

1. One picture of a flow: by its statements, in the editor, the Review, View runs and History's
   dry run.
2. Every place a person decides shows the same rows of what a flow may do, and what changes.
3. Keys and the palette reach everything a pointer reaches.
4. Pages are shown to whoever may read them, and their buttons follow `can`.
5. A run's evidence is on its page: what it read and wrote, the secrets it read, its record, and
   how to check it offline.
6. One place for a flow's settings.

**Non-goals:**

- the People pages (`access-and-approvals-L4.md` §12, step 7);
- approvals in steps (its steps 4 to 6);
- the mockup's Big files tab: a resource's timeline is `host-resources-L4.md` §5;
- notifications;
- any new resource kind.

**Dev phase: no compatibility.** The Tools and Deployment panels go; nothing keeps their layout.

## 3. Principles

These settle every conflict between the two designs:

1. **Text is the source.** Agents write it, the server saves it, reviews diff it. Every picture
   is drawn from it.
2. **Statements first, nodes as their detail.** A row is what a person reads; a node is what the
   core runs.
3. **Where the mockup illustrates and the editor works, the editor's behaviour stays.** That covers
   approvals, resource kinds, setup and invite, the manual, the run list and the marketplace. The
   mockup's layout is taken where it reads better.
4. **One set of rows for "what it may do".** The rows come from the manifest. They are grouped as
   to approve, the same and no longer, each with its line. The may-strip, an Approvals card, the
   Review, the flow's Settings and History's go-back all show them.
5. **Colour has one meaning each.** Amber: a person must act, or it is new. Teal: a call outside.
   Purple: data. Red: failed.
6. **Shown by `can`.** A page is listed if the caller may view it. A button the caller may not use
   is disabled, with the server's refusal as its title.

## 4. The statement graph

**A wasm export, `holon_graph(source, every)`,** beside `holon_usage` (`holon-wasm/src/lib.rs:1268`). It returns
`holon_author::graph::graph`'s `Graph` as JSON. `Graph`, `Band`, `Row` and `Part` gain `Serialize`.

The terminal editor and the web read the same function, so they can't draw a flow differently.

**One component, `StatementGraph`, draws it:**

- **Layout:** one column per band, headed by its trigger. One box per row: its kind word and line,
  its name, and a colour bar.
- **Kinds:** the four `Kind::of_leaf` (`workbench/graph.rs:32`) decides from `does`, and one
  more:
  - **computes** (grey);
  - **reads or writes** (purple);
  - **calls out** (teal);
  - **answers** (accent);
  - **waits** (amber): an `approval` or an event. This one is new, and `of_leaf` gains it so the
    terminal editor draws it too.
- **A fallback answer** (`?? status(404)`) is a side box on its row, "missing → 404". Finding it
  among the row's `parts` is part of step 5.
- **Wires** are `from` within a band, and `outside` as a label.
- **Clicking a row** puts the text cursor on its `mark`. The cursor in the text selects the row
  it is in.
- **New against another version:** given a second graph (the deployed version), rows whose text is
  new are drawn dashed amber. This is how the Review's Canvas tab and the editor's Edit mode show
  what a version adds.
- **With a run:** each row shows the value its last node made, from the run's trace through the
  text link (`text-graph-link-L4.md`). Rows that didn't run are dimmed, and the current step has a
  thick border.

**Where it is drawn:**

- the editor's canvas, at the Statements level (§5.2);
- the Review's Canvas tab, in place of `ReviewPage.tsx:351`'s node SVG;
- View runs;
- History's dry-run review.

## 5. The editor

### 5.1 The workspace

```
[Edit | View runs]  refunds.holon.ts · v3 live · changed   c x v b i   Format Check Save Deploy
┌ It may: + call https://api.stripe.com  · read table orders in shop  ·  against v3: 1 new ─────┐
│ Canvas (Statements | Nodes)   │ Text                         │ Inspector                      │
├───────────────────────────────┴──────────────────────────────┴────────────────────────────────┤
│ Problems 0 · Try a request · Replay the draft · Settings                       (bottom, b)    │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

**The panes:**

- **`c`** canvas, **`x`** text, **`i`** inspector and **`b`** bottom, in both modes; **`v`** values,
  in View runs only.
- One of canvas or text always shows, as today.
- On a phone the pane switcher (`App.tsx:1187`) gains "bottom".
- **The Tools and Deployment panels go.** Their contents move to the bottom pane's tabs (§5.4).

**The bar:**

- the mode;
- the file name, with its state (`v3 live`, `saved`, `draft`, `changed`);
- the pane toggles;
- in Edit: **Format**, **Check**, **Save**, **Deploy**;
- in View runs: **All runs** and **History**, which go to the flow page.

**Pause, resume, stop and run move to the flow page's header** (§8.1). The editor is for the
text, and the bar shows the flow's status as a chip linking there.

### 5.2 The canvas: Statements and Nodes

- **Statements** (the default): `StatementGraph` over the text as typed. It is read-only: it is
  edited through the text, and clicking a row moves the cursor.
- **Nodes:** today's React Flow canvas, with:
  - the primitive Palette, shown only at this level;
  - composites, projection and group / ungroup;
  - undo and redo, tidy and fit, in the canvas's own controls;
  - two-way sync with the text.
- **Switching levels:** the switch on the canvas pane's header, or the key **`l`**. Opening a row
  (Enter, or a double click) shows the Nodes level framed on that row's `parts`.
- The level is kept per tab.

### 5.3 The Inspector

The Inspector follows the cursor in the text, or the row picked on the canvas.

- **On a name the language knows** (`post`, `one`, `route`, `approval`…): its forms, each with
  what it does, from `holon_usage` as today. Then "In the manual →", and an example from a
  sample, with **Open the sample**.
- **On an imported name** (`import shop from "database:shop"`):
  - what it is: a host it may call, a database, a folder, a collection, a secret or a channel;
  - where it is bound on this host, from the resource routes (`resource.view`);
  - for a secret, its mark: "set · v2", or "not set yet". Never its value;
  - what the flow may do through it, each with its line.
- **On a statement:** its kind word, its lines, and "It may, here".
- **Always:** a "This flow" section with its port, and what starts it (the bands' titles).

**At the Nodes level,** the node sections stay as they are:

- params;
- inputs and outputs;
- "as text";
- the unit tree;
- in View runs, the State and Record tabs.

### 5.4 The bottom pane

**In Edit:**

- **Problems**: `holon_mistakes` as you type. The list moves here from under the text
  (`TextPane.tsx:185`). Clicking a row goes to its line.
- **Try a request**: the Tools panel's request tester (send local, send live) and its trigger
  rows:
  - "every …" with **tick now**;
  - an action with what to send, and **run**;
  - "on file", "on mail", "on feed", "on queue".
- **Replay the draft**: for a flow that is deployed, the text as it is now, run on its newest 20
  runs. It uses the same `GET …/replay?unit=&runs=20` the Review uses (`verify/replay.rs:58`),
  after staging the unit (`POST /unit`).
  - It answers a table: run, what came in, was, now, same or differs.
  - A call the deployed version never made has no recorded answer; the row says so.
  - A draft never deployed reads "No runs to replay yet: deploy it first, or try a request."
- **Settings**: the Deployment panel's groups (server, limits, network, permissions, secrets) for
  the version in this tab. They are deployed with it.

**In View runs:**

- **Runs**: today's run list, with its filter, sort, selection, compare, export, ↺ restore and
  ● live.
- **Run**: Details, Errors and Timeline, with the stepper.

### 5.5 View runs

- The may-strip's place shows the run: its number, status, what came in, its version and time, and
  chips for "↗ called out" and "✎ wrote".
- The canvas is `StatementGraph` with the run (§4). At the Nodes level, it is today's node canvas
  with values.
- **The Inspector's Record tab gains "Claims":** whether the run held a key it read until its
  commit, and for how long (the log's `held_ms`, `recorder.rs:270`; `key-claims-L4.md`).

### 5.6 Tabs

- A tab reads `slug.holon.ts`, with ● when changed.
- **Closing a changed tab asks three ways:** **Keep it open**, **Discard**, **Save and close**.
  Today it asks only to discard (`useTabs.ts:145`).

## 6. The shell

### 6.1 Nav and top bar

**Nav:**

- **The host group follows `can`**, in place of `me.role === "admin"` (`Shell.tsx:57`):
  - Resources if the caller holds `resource.view`;
  - People and access if `people.view`, or if they own an agent or a service;
  - Policy if `policy.view`.
- A page reached without the action to change things shows its forms disabled, with the refusal
  as their title.
- Getting started and Docs get the keys **`s`** and **`d`**.

**Top bar:**

- **A Keys button (`?`)** beside the account.
- **The account opens a dialog**, in place of the menu (`AuthGate.tsx:252`–`:255`). It shows:
  - name, kind and role, and the owner for an agent;
  - what they may decide, from `/auth/me/can` (`flow.approve-deploy`, `run.decide`);
  - how they signed in, and since when;
  - buttons: **Change password**, **Getting started**, **Sign out**.

**Narrow screens:** today's breakpoints stay. Under 1180 px the nav is an icon rail, and under
560 px a drawer. The mockup's scrolling nav row is not taken.

### 6.2 Keys

The keys dialog lists them all, from `keymap.ts`, and the status bar names the ones that matter on
the current page. A page's keys come before the anywhere keys.

| Where | Key | Does |
|---|---|---|
| Anywhere | `/`, `Ctrl K` | the palette (`Ctrl K` while typing too) |
| Anywhere | `1`–`8`, `s`, `d` | the pages; Getting started; Docs |
| Anywhere | `t`, `?`, `F6` | Clean / Terminal; the keys; next panel (`Shift F6` back) |
| Anywhere | `Esc` | close; else up to the parent page (run → runs → flow → flows) |
| Lists | `j` `k`, `Enter` | move the selection (kept per page), open it |
| Tabs | `[` `]` | the previous or next tab: a flow page's, the editor's, Approvals' |
| A run | `,` `.` | the run before or after, on the run page and in View runs |
| Approvals | `a`, `d` | approve (opens the review if not read), decline with a reason |
| A review | `n` `N`, `space`, `w`, `d`, `]` | hunks; open or close; whole flow; the diff; the next waiting |
| The editor | `e`; `c x v b i`; `l` | Edit or View runs; the panes; Statements or Nodes |
| The editor | `Ctrl S`, `Ctrl D`, `Shift Alt F` | save, deploy (a review first), format |
| View runs | `j` `k`, `Home` `End`, `=` | step through the run; its ends; values |
| A dialog | a button's letter, `Enter`, `Esc` | press it; the highlighted one; cancel |

`d` is Docs anywhere, Decline on Approvals, and the diff in a review. A page's key wins. Each page
says its own `d` in the status bar.

**The review's hunk keys are already handled** (`Review.tsx`); they join `KEYMAP` so the dialog
lists them. The dialog's letters generalise `Dialog.tsx`'s `y` and `n` to every button.

### 6.3 The palette

**It finds:**

- pages;
- flows, and each flow's tabs except Logs;
- `Edit` and `View` for each flow;
- samples;
- the looks;
- the commands it has today.

**And, new:**

- **each flow's Logs tab**;
- **runs**: the newest across flows (`/summary`'s `latest`, `summary.rs`, ten of them). A typed
  number (`4812`, `#4812`, `run 4812`) offers `run #4812 of <slug>` for each deployed flow; one
  not kept opens the run page's "not kept" state;
- **runs waiting**: `approve run #N of <slug>`, for each a person may decide (`run.decide`), going
  to its row on Approvals;
- **actions**: **Check the run log** (`POST /summary/check`) and **Sign out**;
- **colon commands**: `:check` runs the check and shows Problems; `:call` opens Try a request.
  These join `:12`, `:deploy`, `:save`/`:w`, `:fmt`, `:new`, `:restore`, `:theme` and `:keys`
  (`commands.ts:59`).

**Ranking** stays as `commands.ts` does it, with a run number put first.

### 6.4 Status bar and toasts

- **The Terminal status bar** splits what waits: "waits: 2 changes, 2 runs".
- **A toast may carry one link** ("Open the run", "Open the flow"). A failure stays until closed,
  as today.

## 7. Overview, Approvals, Review

### 7.1 Overview

- **The admin's warnings banner stays first** (`OverviewPage.tsx:88`).
- **"Needs a person":** one row per change waiting. Each row has the slug, "v3 → v4", the
  proposer's chip (accent for an agent, outline for a person), up to two teal chips of new
  permissions, and **Review**. Then the runs waiting, with **Runs waiting**.
- **Tiles** (today `OverviewPage.tsx:145`–`:150`):
  - flows live ("7 of 9");
  - runs in the last 24 h;
  - of them failed;
  - **waiting for a person**.
  The run log moves from a tile to its card on the right.
- **Flows**, with these columns:
  - Flow, with its version;
  - State, with "v4 waits";
  - Starts on;
  - Last 24 h, a sparkline;
  - **p50**: needs `/summary` to answer each flow's median (§11);
  - It may (a count);
  - Last run.
- **Latest runs** as a table, with these columns:
  - Run;
  - Flow;
  - What came in;
  - Status;
  - Took, from `Latest`'s `duration_ms`;
  - When.
- **Right:** the Run log card and "What was done", as today.

### 7.2 Approvals

**Deploys:**

- **A card for each proposal**, with:
  - the slug and "v3 → v4";
  - its place in the queue ("1 of 2");
  - who proposed it, and when;
  - its why;
  - the rows that change: + new (amber) and − no longer, each with its line;
  - "N the same".

  The rows are the Review's (`reviewMay.ts`), read when the card scrolls into view.
- **The other kinds stay as rows:** a first deploy, a version saved since, a port, can't run, and
  an agent's ask (`approvals.ts`).
- **Buttons:** **Decline with a reason…** and **Review**.

**Runs waiting** gains two columns:

- **What it is**: the route or kind, the approval point's name, who started it;
- **Expires**: separated from "Waits for".

The k-of-n count and "you approved" stay. When approvals in steps land (access steps 4 to 6),
the row shows the chain as `access-and-approvals-L4.md` §12 draws it.

**Keys:** `a`, `d`, `j` `k`, `Enter`.

### 7.3 Review

Kept as it is, plus:

- **"While it runs"**, under What it may do:
  - its port;
  - its run budget;
  - the runs it keeps;
  - what it holds between runs: its state, its claims.
- **Host policy**: a ✓ row for each policy line this version relies on (a host it calls is
  allowed, agents may propose, the caller may approve), from `GET /permissions` and `can`.
- **A note, kept with the grant**: needs `POST /deploy` to take one, and the grant's event to keep
  it (§11).
- **Open in the editor**, in the header.
- **The Canvas tab** is `StatementGraph` with this version's new rows dashed (§4).

## 8. A flow

### 8.1 Header and tabs

**Header:**

- the state dot;
- the slug;
- the version, or "draft";
- the state chip;
- what starts it ("2 routes · :8110");
- its first comment line, as its description.

**Buttons:**

- **Open in editor** (`e`);
- **Review v4** (amber, when one waits);
- **Pause** / **Resume**;
- **Run now**, for a flow with a clock or an action.

Stop and Remove move to Settings (§8.4).

**A draft never deployed** shows "It would be allowed to", with the rows from its manifest and
**Deploy…**.

**Tabs:** Status, Runs, **Logs** (kept), History, State, Settings.

### 8.2 Status, Runs and a run

**Status:**

- The Versions card lists a version that waits first, with "waits for a person", who proposed it,
  and **Review →**.
- It may gets **Change…**, which goes to Settings.
- The p95 tile and the action rows stay.

**Runs:**

- columns Run, When, What came in, Status, Took, Did outside, **What went out**;
- **Check the chain** opens a dialog in place of the inline line;
- the tally rows stay.

**A run's page:**

- **Secrets it read**: each one's name, version, and where it went, from the run's record. The
  value is never shown.
- **On record** gains **Witnessed**: when a witness took a head at or after this run (§12, Q7).
- **Check offline…** names the commands for this run. Exporting one run needs the server (§11).
- The log lines, session and "ran again" stay.

### 8.3 History and State

**History:**

- **Before the dry run, "Go back to here"** shows a summary in four tagged lines:
  - **code**: "v3: `fee = 5` → `fee = 0.5`", or "+a · −d lines", or "the same";
  - **grant**: checked against today's; "asks for more, so a person approves", or "nothing new";
  - **state**: "tea: 40 → 3, in a new session", or "the same";
  - **since**: "N runs since; their records stay".
- It also shows the banner "Not put back, by design": the calls made, and the files written
  (`history-L4.md` §5).
- The dry run is then the same review in hunks as a deploy, with the state hunks.
- The kind toggles stay.
- **A picked state cell gets a line over time,** from its changes (`CellHistory`), with the
  restore point marked.

**State:**

- A value's history is shown inline (⧗ N values), in place of the dialog.
- **Files it wrote**: each file the flow's runs wrote, how many runs wrote it, and the last one,
  from the runs' effects.

### 8.4 Settings: one place

Every setting of a deployed flow is shown here, edited in place:

- what it may do, with **Revoke** and **Grant back** (today);
- secrets;
- limits: calls out per minute, run budget, request body size;
- runs kept: newest, days;
- network;
- whether agents may propose;
- sessions;
- waiting runs.

**How a change applies:**

- **Lowering** a limit, or keeping fewer runs, applies at once. Today this works for limits
  through the grant.
- **Raising** one changes what the flow may do, so it is a new version through the deploy review.

"Changed in the editor (⚙), by deploying it again" (`FlowSettings.tsx:293`) goes. The editor's
Settings tab (§5.4) is for a version not deployed yet.

**Lifecycle:**

- **Pause** / **Resume**;
- **Stop…**: says how many waiting runs would end;
- **Remove…**: type the slug; its run records stay.

## 9. The host pages

### 9.1 Resources

**The tabs stay** (`HostPages.tsx:707`): Databases, Folders, Mail, Feeds, Queues, Secrets, Host and
storage, Who may touch what. The bind forms stay. Each tab becomes a table with:

- **Used by**: each deployment, and what it does with the resource. A version waiting for a person
  shows what it would add, in amber ("stock-sheet v2 would write"), read from the proposals'
  manifests.
- **For a database, a matrix**: tables × the flows that use them, with read, write or both; amber
  for a waiting version. It replaces "Who may touch what" for databases. That tab stays for the
  other kinds.
- **Collections and channels**, a new tab: each flow's collections and channels, with how many
  items each holds, and a link to the flow's State.
- **Secrets**, as a table:
  - name;
  - version;
  - where its value comes from (a value sealed, `env:`, `vault:`);
  - set: when, and by whom;
  - where it may go;
  - which flows read it;
  - **Set a new value** and **History**.

### 9.2 People and access

This page belongs to `access-and-approvals-L4.md` §12 (step 7). It already lists principals of
three kinds, their owners, tokens, roles and bindings (`HostPages.tsx:488`; `access.ts`).

From the mockup, it takes three columns:

- **Last seen**, for a person;
- **Last used**, for a token (§11);
- a **Connect it** line on each agent: today's `holon mcp setup` line.

### 9.3 Policy

Kept as it is (`HostPages.tsx:1811`), plus:

- **The policy file**, shown as text with line numbers. `GET /permissions` needs to answer it
  (§11).
- **A "What it means" column** on the limits table: one sentence each. Neither page has them
  yet; they are written once, and the editor's Settings tab (§5.4) shows the same ones.
- **Check flows against it** also checks the versions waiting for a person: "refunds v4 calls
  api.stripe.com, which `hosts` allows".
- **Held off** stays.

## 10. Help

**Getting started:**

- Its steps tick themselves, as today (`guide.ts:9`), and each shows the command that does it
  from a terminal (`holon new hello-api.holon.ts --sample hello-api`, `holon mcp setup …`).
- One name everywhere: nav, palette, account dialog and title. Today: "Getting started",
  "Welcome", "Welcome and docs".
- The stale copy goes: "the server panel at the bottom left" (`guide.ts:23`), and the Marketplace
  set "on the flows page".

**Library:**

- Tutorials are the manual's guides, each card with its first paragraph and the samples it uses.
- Samples and Marketplace stay.

**Docs:** stays.

## 11. Server and wasm changes

Everything else is the editor's, over routes that exist.

| Change | Where | For |
|---|---|---|
| `holon_graph(source, every)`; `Serialize` on `Graph`, `Band`, `Row`, `Part` | beside `holon-wasm/src/lib.rs:1268`; `holon-author/src/graph.rs` | §4 |
| a flow's median run time (`p50_ms`) in each `FlowSummary` | `summary.rs:56` | §7.1 |
| a note in `POST /deploy`'s body, kept on the grant's event | `deploy.rs`, `events.rs` | §7.3 |
| `text`: the policy file as read | `deploy.rs:819` `permissions_view` | §9.3 |
| last sign-in per person, last use per token, written at most once a minute | `accounts.rs` | §9.2 |
| settings that lower, in place: `POST …/settings` beyond `agents_propose` | `proposals.rs:298` | §8.4 |
| one run's export, and `holon verify` of it | `verify.rs:95`, `verify.rs` | §8.2; Q3 |

## 12. Open questions

1. **A flow built on the canvas.** A flow with composites placed: is its Statements level still
   the default? Proposed: yes; a placed holon is one row, and opening it shows its nodes.
2. **Which settings change in place.** Does raising a limit need a new version, or only an
   approval of the same version's grant?
3. **One run, checked alone.** Is a run checkable offline from its own line and the hash before it,
   against a witnessed head, without the chain before it? If not, "Check offline" exports the log
   up to the run.
4. **What a viewer sees of a resource.** A database's connection source or a folder's path: shown
   to `resource.view`, or only to `resource.bind`?
5. **Last seen on the record.** A hint on the People page, or an event in the activity log?
6. **`d` in three meanings.** Docs, Decline and the diff. Is a page key shadowing a global one
   clear enough with the status bar saying it?
7. **Witnessed, per run.** Heads go to the witness in batches (`/summary`'s `witness`). Does the
   server keep which batch covered a run, or is "Witnessed" only known for the newest head?

## 13. Tests

**vitest:**

- `holon_graph` answers bands for every sample, and `StatementGraph` draws them (with
  `samples.test.ts`, every sample).
- A new row against another version is dashed.
- A row's click moves the cursor.
- `KEYMAP` has a handler for every key it lists, and the dialog lists every handled key.
- The palette finds a run by number.
- A button the caller may not use is disabled, with the refusal.

**cargo:**

- `holon_graph`'s JSON round-trips `graph()`'s output for every sample.
- `p50_ms` in `/summary`.
- The deploy note on the grant's event.
- The policy text.
- Last use is written at most once a minute.

**e2e** (`editor/e2e`):

- A sample opens at the Statements level.
- Clicking a row moves the cursor.
- Replay the draft on a deployed sample.
- An Approvals card's rows.
- A viewer sees Policy read-only (the built-in viewer holds no `resource.view`; a role that
  holds it alone sees Resources so).
- The keys `j` `k` `Enter` on Flows, and `a` on Approvals.
- Closing a changed tab with Save and close.

**No new sample.** This is the console over features that each have one. The statement graph is
checked over every sample, and the e2e opens one.

## 14. Steps

One commit each. Steps 1 to 4 need nothing from the server but what §11 lists beside them.

1. **Keys and palette.**
   - The Keys button and the account dialog.
   - `s` and `d`; lists' `j` `k` `Enter`; `[ ]`; `,` `.` on a run; letters on dialog buttons;
     the hunk keys in `KEYMAP`.
   - The palette's runs, runs waiting, Check the run log, Sign out, `:check`, `:call`.
   - The status bar's split; toast links.
   - One name for Getting started; the stale copy.
2. **Shown by `can`.** The host group and its pages for whoever may view them; disabled buttons
   with the refusal, on every page. This is access step 7's "buttons from `can`", done here for
   the pages this note owns.
3. **Overview and Flows.** Tiles, columns and the latest runs table, with `p50_ms`.
4. **Approvals and Review.** Proposal cards, the Runs waiting columns, While it runs, Host policy,
   the grant's note, and Open in the editor.
5. **The statement graph.** The wasm export, `StatementGraph`, and the Review's Canvas tab.
6. **The editor's canvas.** Statements and Nodes, `l`, and the Inspector's three cases with "This
   flow".
7. **The editor's bottom pane.** Problems, Try a request, Replay the draft and Settings in place of
   the Tools and Deployment panels; the bar; lifecycle to the flow page; closing a changed tab.
8. **View runs on the statement graph,** and Claims.
9. **A flow's pages.** The header, Status's waiting version, Runs' What went out and its chain
   dialog, the run's secrets and witness, History's summary and chart, State's inline history and
   files, and Settings in one place (with `POST …/settings`).
10. **Resources and Policy.** Used by with waiting versions, the database matrix, Collections and
    channels, the secrets table; the policy text, What it means, and the check of waiting versions.
11. **Help.** Getting started's commands; the Library's tutorials.
12. **Last seen and last used,** server and columns. Or folded into access step 7, whichever comes
    first.

Each step that changes what the editor shows runs `npm run e2e` before its commit, or its pull
request is labelled `e2e` (`CLAUDE.md`).
