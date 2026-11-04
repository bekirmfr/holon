# Components: panes of the editors, written in the language, that can do nothing on their own (L4)

**Status:** design (2026-11-03). Nothing here is built. Written beside docs/L4s/tours-L4.md, which
leaves components to this note (its fork 10). Claims about the code as it is are cited by file.

The editors show what every flow has: its runs, its history, its state, what it may do. What a
particular flow *means* is nowhere: the refund desk's refunds paid today, what waits for a manager,
the chess game's board. A flow can serve a page of its own (`gemini-chat`, `guestbook`, `chess`),
but that page is outside the editors, on the flow's port, for its users; nothing of it reaches the
person working in the editor, or the terminal at all. This note gives a flow a pane in the editors:

> **A component** is a `.holon` file whose header says `component` where a flow's says `flow`. Its
> one handler, `view`, takes data the editor fetches for it, as the person, and answers a
> description of a pane, in a small vocabulary both editors draw: text, tables, figures, a grid,
> buttons. It runs in the editor, not on the server, and it has **no permissions at all**: no
> network, no state, no secrets, no writes. A button in it runs a flow's action only when the
> person clicks it, and the editor shows which action, whatever the button says.

Two consequences carry the design. A component can be written by anyone, an agent included, and
opened without fear: the worst it can do is draw something misleading, and what its buttons do is
drawn by the editor, not by it. And the same component is drawn in the browser and in the terminal,
because it describes a pane rather than drawing one.

## 1. What is already there

- **Pure code runs in both editors.** The wasm bridge's `Evaluator` wraps "the *same*
  `MapResolver` and `eval` the native host uses — there is no second evaluator"
  (`crates/holon-wasm/src/lib.rs`), and design mode runs flows with it. js holons run in the
  browser too: the bridge carries `holon-module`, "one implementation, compiled twice: native for
  the host … and wasm32 for the editor's bridge" (`crates/holon-module/src/lib.rs`), with the Boa
  engine under a fuel and memory budget (docs/L4s/js-node-L4.md §4). Its prelude removes the clock,
  randomness, timers and the network.
- **Permissions are derived, not declared.** `capabilities_of` gives each leaf its capabilities
  (`crates/holon-core/src/kind.rs`); a flow's manifest is the union over its graph. A unit whose
  manifest is empty can do nothing outside itself, by construction.
- **What the editors read, as the person.** A deployment's detail, state, collections, runs and
  history (`workbench/client.rs`: `detail`, `state`, `collection`, `run_view`, `changes`;
  `editor/src/server.ts`). Each is redacted for secrets already (secret-destinations-L4.md §1).
- **Running an action as the person**: `POST /trigger/<slug>/<name>`, from both editors
  (`client.rs::action`, the web editor's action rows, `ActionRows.tsx`).
- **Where a flow's own view would go.** The terminal editor's deployment page has "a head with its
  state and buttons, the sections, then the dashboard" (`workbench/ui/flow.rs`); the web editor's
  view mode has panes around the canvas (`ViewPanes.tsx`). Neither has room for anything a flow
  defines.
- **Saved flows.** `POST /flows` keeps a document under a name, without deploying it (`flows.rs`);
  both editors list them.

## 2. A component, as a file

```holon
component desk-dashboard
title "Refund desk"
for desk

# What the editor fetches for the view, as the person looking.
reads state of desk
reads collection refunds of desk limit 200
reads runs of desk waiting

# Today's refunds, split: what was paid and what was refused.
holon today(refunds, now) -> split js {
  const day = Math.floor(now / 86400);
  const all = Object.values(refunds).filter((r) => Math.floor(r.at / 86400) === day);
  return { paid: all.filter((r) => r.paid), refused: all.filter((r) => r.refused) };
}

view (data) {
  t = today(data.refunds, data.now)
  paid = t.paid
  refused = t.refused
  waiting = map(data.waiting, fn: fn(w) -> [
    w.input.order,
    "{w.input.amount} €",
    button("Approve", run: "desk.approve", input: { run: w.run }),
  ])
  respond column([
    figures([
      { label: "Paid today", value: sum(map(paid, fn: fn(r) -> r.amount)), unit: "€" },
      { label: "Refused today", value: length(refused), tone: "warn" },
      { label: "Waiting for a manager", value: length(data.waiting), tone: "warn" },
    ]),
    heading("Waiting"),
    table(["Order", "Amount", ""], waiting),
    heading("Refused today"),
    list(map(refused, fn: fn(r) -> text("{r.order}: {r.reason}", tone: "bad"))),
  ])
}
```

Only the header lines, `view`, and the vocabulary of §4 (`column`, `figures`, `button`, …) are new;
the rest is the language as it is, holons and js holons included.

### The header

| Header | Meaning |
|---|---|
| `component NAME` | In place of `flow NAME`. |
| `title "…"` | What its tab or pane is called. |
| `for SLUG` | The deployment it belongs to: its pane shows on that deployment's page in both editors. Without it, a component is shown only where something names it (a tour's `show component NAME`, §6). |
| `reads state of SLUG`, `reads collection NAME of SLUG [limit N]`, `reads runs of SLUG [waiting] [limit N]`, `reads deployment SLUG` | What the editor fetches for `view`, through the API it already uses, as the person: `data.state`, `data.<name>`, `data.runs` / `data.waiting`, `data.deployment`. |

`data.now` is the time the editor drew it, the only thing a view gets from outside its reads. A
component has no `secret`, `state`, `collection`, `folder`, `database`, `calls`, `port` or other
handler: those are a flow's.

## 3. Nothing on its own

A component's unit must have an **empty manifest**. The compiler checks it as it derives any flow's
(`capabilities_of`), and a component that uses a leaf with a capability is a mistake at that line:
"a component can't call out: a component only draws; a flow does this, and the component shows its
result". So:

- **No network.** No `http.*`, no flow calls; js holons in it run under the prelude that has no
  `fetch` (js-node-L4.md §4).
- **No state, no writes, no secrets.** What it shows comes only from its `reads`, which the editor
  fetches as the person looking: a component can't show anyone what they couldn't open themselves.
  Secrets never reach it: what the editor reads is redacted already.
- **No session.** The view runs in the core's evaluator, in the editor's process, and gets values,
  never the person's token or the editor's API.
- **Bounded.** A view runs under a fuel and memory budget like any js holon (`fuel`, `memory` in
  its header; defaults small); over it, the pane shows the error, not a frozen editor.

What a component *can* do is ask, in its description, for a button. §5 is how that stays the
person's act.

## 4. The vocabulary

`view` answers a value made of these, each a record with a `ui` field (the functions above build
them; a record written by hand is the same). Both editors draw every one; nothing else is drawn.

| Piece | What it is | The web editor | The terminal editor |
|---|---|---|---|
| `text(s, tone:)` | A line of text; tone `plain`, `dim`, `good`, `warn`, `bad` | Styled text | The editor's own tones (`Tone`, `workbench/app.rs`) |
| `heading(s)` | A section's title | A heading | A head line |
| `figures([{label, value, unit, tone}])` | A row of numbers that matter | Stat tiles | One line of `label value`, toned |
| `table(columns, rows)` | Rows of cells; a cell is text or a piece | A table | Aligned columns, scrolling |
| `list(items)` | Items, one a line | A list | Lines |
| `grid(rows, cells)` | A grid of short cells, each `{text, tone, button}` | Square cells | Cells two or three wide |
| `button(label, run: "SLUG.action", input:)` | An action the person may run (§5) | A button | A key-selectable button |
| `link(label, to:)` | A place in the editor (`run SLUG ID`, `deployment SLUG`, `flow SLUG PATH`) | A link | `enter` goes there; a flow's page offers the browser |
| `code(text)` | Holon text, highlighted | The text pane's highlighting | The editor's highlighting |
| `column(items)`, `row(items)`, `section(title, items)` | Layout | Flex boxes | Stacked; a row wraps when narrow |

The `grid` is there for boards of every kind: a chess position, a seat map, a week of shifts. The
chess sample's board, drawn by a component from the game's state, plays the same in both editors.

A description that isn't one (an unknown `ui`, a missing field, a button with no `run`) is shown as
a mistake in the pane, with the path into the value, and nothing of it is drawn. There is no HTML
and no style: a component looks like the editor it is in, in both themes, and can't draw over the
editor's own controls (§9, fork 2).

## 5. Buttons and links are the person's

- **A button names an action, as text, and the input it sends.** The editor draws the button with
  the component's label **and its own line beneath: the action it runs and on which deployment**
  ("runs `desk.approve`"), from the button's `run`, not from the label. A button labelled
  "Refresh" that runs `desk.refund_all` says so where the person reads it.
- **Clicking it runs the action as the person**, `POST /trigger/desk/approve` with the input, as the
  action rows do; it is in the activity and run logs as theirs, with "from the component
  desk-dashboard". An action the person can't run is drawn disabled, saying why. An agent's token
  never clicks: a component has no way to run anything that isn't a click.
- **Links go only to places in the editor and to the `for` deployment's routes.** A component can't
  make a link to anywhere else, so it can't send what it read out of the editor in a URL a person
  might click.

## 6. Where a component shows

- **On its deployment's page**, `for desk`: a tab beside the editor's own in the web editor's view
  mode, a section of the terminal editor's deployment page (after its dashboard). Several
  components for one deployment are several tabs.
- **In a tour**: `show component desk-dashboard` in a step (tours-L4.md §2) opens it beside the
  rail, so the refund desk tour shows the refunds and the waiting queue as it goes, and the chess
  tutorial shows the board.
- **Opened as a file**, in either editor's text pane, with a preview against the data it reads
  (with a sample `data` when there's no server), as a flow's text has its graph beside it.
- **Refreshed** when the editor reads its data again, as it refreshes the deployment's page (the
  web editor's view mode follows the flow log live, `ViewPanes.tsx`). A view is computed again
  only when its data changed: the same data, the same pane, since it is pure.

## 7. Where components are kept

A component is kept on the server like a saved flow (`POST /flows`, `flows.rs`), as a document whose
header says `component`, so every person with the editor sees the same panes. Saving one needs no
approval: it can do nothing (§3). Removing one is a click, as for a saved flow. Its versions are kept
as a saved flow's are.

A component that belongs to a sample or a tour lives beside it (`crates/holon-author/samples/`, a
tour's folder) and is saved with it when the sample is opened or the tour played.

## 8. Tests and samples

- **Every component compiles with an empty manifest**, in holon-author's tests, and is kept as
  `fmt` writes it.
- **Each one has a `data` fixture and its rendering**: `holon component draw desk-dashboard.holon
  --data data.json` draws it as plain text (the terminal editor's drawing, without colour). The
  rendering is checked in as a snapshot; a change to the view or to a drawing shows in its diff.
- **The e2e tests open each sample's component** on its deployment's page, and click one button.
- **Samples:** the refund desk's dashboard (figures, a table, approve buttons); the chess board (a
  grid whose cells are buttons that run the chess flow's move action), which shows the same
  component in both editors. Each counts as the sample of what it shows (CLAUDE.md).

## 9. Forks, surfaced

1. **Components in the language, or in TypeScript and Rust?** In the language. A pane in
   TypeScript reaches the web editor only, one in Rust the terminal only; written once in the
   language, it is drawn by both, can't do anything, and is checked, formatted and served like any
   `.holon` file. The editors' own panes stay theirs.
2. **HTML in a component?** No. HTML would draw in one editor only, bring its own style and
   scripts, and could imitate the editor's controls (a fake "approve" over the real one). The
   vocabulary is small on purpose; what it can't draw belongs on the flow's own page.
3. **Reads of any deployment, or only the `for` one?** Any, each named in a `reads` line, fetched as
   the person: a dashboard across the desk, the bank and the model is the point of some panes. The
   tour's components read what the tour shows.
4. **Where the view runs: the editor, or the server?** The editor. The server would need to read
   as the person and run code on each look; the editor already holds the data and the evaluator,
   and a view's cost lands with the person looking.
5. **Buttons that only change the pane (sort, expand, pick a tab)?** Not at first: they need a
   view's own state. A later slice can give `view` a second input, `ui`, the pane's own small state,
   kept by the editor, never the server's.
6. **The editors' own panes as components?** Not now. Some (a deployment's dashboard) could be one
   day, which would show the vocabulary is enough; the editors' panes that approve, deploy or change
   anything never will be.
7. **Holons shared between a flow and its component** (the chess rules drawing the board and
   checking moves)? Copied for now, as the language has no imports; a shared holon file is its own
   note.

## 10. Slices

1. **The language.** `component`, `title`, `for`, `reads`, `view`; the empty-manifest check and its
   mistake; the vocabulary as values with their validation, in holon-author; `holon check`,
   `fmt`, `lsp`, the grammar.
2. **The plain drawing and its tests.** `holon component draw … --data`, the snapshot tests; the
   desk dashboard and the chess board as samples, with fixtures.
3. **The web editor.** Fetching the reads, running the view in the bridge's evaluator, drawing the
   vocabulary in both themes, the deployment page's tabs, buttons with their line, links; saving
   and listing components (`/flows`).
4. **The terminal editor.** The same, on the deployment page and as a tab, keys for buttons and
   links.
5. **Tours.** `show component` in both players (tours-L4.md).
6. **A pane's own state** (fork 5), when a sample needs it.
