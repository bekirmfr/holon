# Approvals in steps, the language and core: implementation plan

**Companion to** `access-and-approvals-L4.md`, the design, and to `principals-plan.md`,
`can-and-catalogue-plan.md` and `roles-and-bindings-plan.md`, its steps 1 to 3. This file says how
to build the design's §14 step 4:

- an approval in steps in the language: slots, each with a role, a kind and how many people, in
  order or not (§8.1);
- the canonical text of the steps, the `approval` leaf's params, and its capability (§8.2);
- the short form, `people: N`, as one slot of that approval (§8.1);
- the value the run receives (§8.1).

Step 5 is the server's: deciding by slot, separation of duties, notes, the listing, the timeline
and the evidence, `holon decide --slot --note`, the MCP `decide` tool. §6 says where step 4 leaves
it, and §7 plans it.

Each sub-step names the files, functions and lines it changes at `d289e435` (main, rc.35). It
sketches the new code, lists the tests it changes and adds, and gives the checks to run. Code here
is a sketch of shape and names, not final code. Where the design leaves a decision open, or the
code raises one the design doesn't, the sub-step names it (§D, from D29, after step 3's D28).

## 0. Conventions

The conventions of the earlier plans hold (`principals-plan.md` §0 and the ones after it), with
these additions:

- **Three sub-steps, one commit each.**
  - 4a gives the `approval` leaf its canonical steps in place of `people`. The capability becomes
    `Approval("payout/person*2")`, and the server decides the short form as it does now. Nothing a
    flow can write changes.
  - 4b adds the steps to the language: `steps`, `order`, stages, and each slot's `role`, `kind` and
    `people`. They are parsed, lowered, printed back, typed in `holon.d.ts` and documented. Until
    step 5, the server refuses at deploy an approval it can't decide yet (D31).
  - 4c gives the run each decision as a record, with its slot, kind and role (D30).
  - Each sub-step's docs go in its own commit: the language (`docs/LANGUAGE.md`, the manual's
    language reference and tour), `holon.d.ts`, the server reference where it changes, the
    CHANGELOG.
- **Steps 4 and 5 go on one branch and one pull request** (D31): what a flow can write in 4b, the
  host decides in step 5. Each commit passes every check on its own.
- **One model of the steps, in core.** `holon_core::approval::Steps` parses the canonical text,
  writes it, says it in words and limits it. The language, the capability, the host, the server,
  the review and wasm read it there, and nothing else parses the text.
- **Class-level tests** (the test-suite review's rule), over one generated universe of steps
  (§T): the canonical round trip, the language round trip, the refusals as a table, and the
  capability telling every two different steps apart.
- **Checks**:

  | Sub-step | cargo test, clippy | wasm-pack build and test | tsc, vitest | `npm run e2e` (slot 3) | Bless |
  |---|---|---|---|---|---|
  | 4a | yes | yes: the leaf's params change | yes | yes: `approved-payout` deploys and is decided by two people | `leaves.md` (the leaf's params and doc) |
  | 4b | yes | yes | yes | yes: every sample still deploys | `holon.d.ts`; the samples if fmt changes them |
  | 4c | yes | | yes | yes: `approved-payout` pays with who approved | `holon.d.ts` |

  `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` each time.
  `wasm-pack test --node crates/holon-wasm` after each catalog change (cargo skips it). e2e runs
  locally with `HOLON_E2E_SLOT=3`, and the pull request gets the `e2e` label.
- **Run live** against `target/release/holon.exe` on scratch port 7530 (flows on 8230 up):
  - 4a: `approved-payout` deployed, its review saying "approved by 2 people", a payout decided by two
    people;
  - 4b: a flow with an approval in steps checked (`holon check`) and printed back as written; its
    deploy refused, saying why;
  - 4c: a payout approved, its run answering the decisions with their slot and kind.

  Each commit says what was run.
- **Samples** (CLAUDE.md). The feature is approvals in steps, and a host decides one only from
  step 5. So `approved-payout` moves to the chain in step 5, when it can be run live. Its
  canonical capability changes in 4a, and 4c changes what its run is given. 4b's commit says the
  sample comes with step 5, and why.

| Sub-step | Change | Where | Decisions |
|---|---|---|---|
| 4a | `holon_core::approval`; the leaf's `steps` param; `Approval("name/steps")`; the host's `Waiting`; the short form lowered and printed from it; the review's words | `holon-core`, `holon-host`, `holon-author` (`lang`, `describe.rs`), `holon` (`durable.rs`, `touches.rs`, `permissions.rs`) | D29, D33, D35 |
| 4b | The steps in the language; `holon.d.ts`; hover docs; the docs; the refusal until step 5 | `holon-author` (`lang/ts.rs`, `lower.rs`, `print.rs`, `editing.rs`, `holon.d.ts`), `holon` (`permissions.rs`), docs | D31, D32, D34 |
| 4c | The decision as records: `decisions` | `holon` (`durable.rs`), `holon.d.ts`, docs | D30 |

## 1. Where an approval is today, at `d289e435`

`await approval("payout", { expires: "1d", people: 2 })` goes through:

| Stage | Where | What |
|---|---|---|
| Parse | `holon-author/src/lang/ts.rs:3752` `event()` | options `expires` and `people` (1 to 5) into `ExprKind::Await { name, expires, people: Option<u32> }` (`ast.rs:391`) |
| Lower | `lang/lower.rs:2097` | `people` set: the `approval` leaf with `name`, `expires`, `people`; unset: `await` |
| Print | `lang/print.rs:3345` | the leaf back to `await approval(…)`, `people` written from 2 |
| Leaf | `holon-core/src/catalog.rs:569` | params `name`, `expires` (604800), `people` (1), all frozen; value a record |
| Capability | `holon-core/src/kind.rs:142`, `:409`, `approval_grant` at `:572` | `Approval("payout/2")`, a driver, sourced; `None` (fail closed) when the name or `people` doesn't parse |
| Request | `holon-core/src/primitives.rs:1834` `approval_request`, `eval.rs:382` | `{await, expires, people}` |
| Host | `holon-host/src/rounds.rs:167` `Waiting`, `:375` | `people` read from the request; 0 is an event |
| Park | `holon/src/durable.rs:252` `StoredWaiting` | `people`, and `approved: [{by, at, reason}]` |
| Decide | `durable.rs:832` `decide`, `:888` `decider`, `:899` `handle_post_decide` | `can(run.decide)` at the point; N different people approve, one decline (with a reason) ends it |
| Value | `durable.rs:287` `StoredWaiting::decision` | `{approved, by: [names], approvals: [{by, at, reason}], declined_by?, reason?, expired?}` |
| Words | `holon-author/src/describe.rs:65`, `holon/src/touches.rs:50`, `editor/src/decisions.ts:32`, `holon-core/src/docs.rs:202`, `editing.rs:88`, `:220` | "wait for 2 people's approval", "2 people approve" |
| Types | `holon-author/src/holon.d.ts:273` (and the samples' copy, `types.rs`) | `approval(name, { expires?, people?: 1…5 }): Promise<Decision>`; `Decision { approved, by, declined_by?, reason?, expired? }` |
| Limits | `durable.rs:42` `awaits_in` | each wait's `expires` held to `max_await_seconds` (reads `name` and `expires` only) |

The run's starter isn't kept on the parked run (`ParkedRun`, `durable.rs`). Step 5 needs it, for
"not the starter" (§8.4).

## 2. The model: `holon_core::approval`

A new module, `crates/holon-core/src/approval.rs`. It holds what `MAX_APPROVAL_PEOPLE`
(`primitives.rs:1829`) holds now.

```rust
/// An approval's steps: stages in order, each a set of slots filled in any order (§8.1).
pub struct Steps { pub stages: Vec<Vec<Slot>> }

pub struct Slot {
    pub kind: SlotKind,          // Person | Agent; a service never fills one (§8.4)
    pub role: Option<String>,    // None: anyone holding run.decide here (the short form, D33)
    pub people: u8,              // 1 to MAX_PEOPLE different principals
}

pub const MAX_SLOTS: usize = 5;  // in all, across the stages
pub const MAX_PEOPLE: u8 = 5;    // in one slot

impl Steps {
    /// `people` people, any holding run.decide: `{ people: N }`, and `approval("x")` with N = 1.
    pub fn people(n: u8) -> Steps;
    /// The steps a canonical text writes; None for any text that isn't canonical (D29).
    pub fn parse(text: &str) -> Option<Steps>;
    /// The canonical text: parse(s.text()) == Some(s), always.
    pub fn text(&self) -> String;
    /// In words, for the review and the agent tools.
    pub fn said(&self) -> String;
    /// How many people, when it is the short form (one slot, a person, no role).
    pub fn short(&self) -> Option<u8>;
    /// Each slot with its number (0-based, in the order written) and its stage.
    pub fn slots(&self) -> impl Iterator<Item = (usize, usize, &Slot)>;
}
```

**The canonical text** (D29). Stages are joined by `>`, the slots of a stage by `+`. A slot is
its kind, then `:role` when it has one, then `*N` when N is more than 1:

| Written | Canonical | Capability |
|---|---|---|
| `approval("payout")` | `person` | `Approval("payout/person")` |
| `{ people: 2 }` | `person*2` | `Approval("payout/person*2")` |
| `steps: [{ role: "classifier", kind: "agent" }, { role: "reviewer" }, { role: "approver" }]` | `agent:classifier>person:reviewer>person:approver` | `Approval("payout/agent:classifier>person:reviewer>person:approver")` |
| the same, `order: "any"` | `agent:classifier+person:reviewer+person:approver` | … |
| `steps: [[{ role: "a" }, { role: "b" }], { role: "c", people: 2 }]` | `person:a+person:b>person:c*2` | … |

Slots keep the order they are written in: their numbers are the `slot` a decision names. So two
texts that differ only in the order of one stage's slots are different capabilities. Reordering
them asks for approval again, which is harmless.

`said()`:

It is a noun phrase, and its callers add the verb: the review's row "approved by 2 people", the
permission's "wait for approval "payout" by 2 people, and continue later".

- `person` is "a person".
- `person*2` is "2 people".
- `agent:classifier>person:reviewer>person:approver` is "an agent holding classifier, then a
  person holding reviewer, then a person holding approver".
- `person:a+person:b>person:c*2` is "a person holding a and a person holding b, in any order, then
  2 people holding c".

Role names follow the rule an approval's name follows (`kind.rs:578` `ident`): letters, digits,
`-` and `_`, at most 64 (D35). So none of `>`, `+`, `:` or `*` can be in one.

## 3. Step 4a. The canonical steps, the leaf and the capability

Nothing a flow can write changes: the short form lowers to `person*N`. What changes is the leaf's
param, the capability's text, the host's waiting point, and the words.

### `holon-core`

- `approval.rs`, as §2. `lib.rs` has `pub mod approval`, and no re-export: `holon_core::Slot` is
  already the evaluator's, so these are `holon_core::approval::{Steps, Slot, SlotKind}`.
- `catalog.rs:569`: the `approval` leaf's params are `name`, `expires` and `steps` (frozen,
  default `"person"`). `people` goes.
- `kind.rs:572` `approval_grant`: `Steps::parse(steps)` or `None`, so a hand-edited canvas param
  that isn't canonical fails closed, as a bad name does. It returns `format!("{name}/{}",
  steps.text())`. `kind.rs:139` says the new form in its doc.
- `primitives.rs:1834` `approval_request`: `{await, expires, steps}`, with `steps` as the
  canonical text. `MAX_APPROVAL_PEOPLE` goes (now `approval::MAX_PEOPLE`).
- `docs.rs:202`: the leaf's doc says `steps`: "the steps people (and agents) take to decide it,
  written as the language writes them: `person*2` is two people".

### `holon-host`

- `rounds.rs:167` `Waiting`: `people: u64` becomes `approval: Option<String>`, the canonical
  steps (`None` for an event, which a token delivers). `:375` reads `steps` from the request.

### `holon-author`

- `lang/ast.rs:391`: `ExprKind::Await { name, expires, approval: Option<Steps> }`.
- `lang/ts.rs:3775`: `people: N` builds `Steps::people(N)`, and `approval("x")` with no options
  builds `Steps::people(1)`.
- `lang/lower.rs:2097`: an approval's leaf gets `("steps", steps.text())`.
- `lang/print.rs:3345`: reads `Steps::parse(node.param("steps"))`. When `short()` is `Some(n)`,
  it writes `people: n` for n from 2, and nothing for 1. Anything else is "has no TypeScript form
  yet" until 4b.
- `describe.rs:65`: "wait for approval \"payout\" by {said}, and continue later".
- `editing.rs:88`, `:220`: the hover for `people` stays; `steps` is 4b's.
- `agent.rs` and `commands.rs` tests that pin `payout/2` take `payout/person*2`.

### `holon`

- `durable.rs:252` `StoredWaiting`: gets `steps: String`, the canonical steps. `people` stays,
  set from `Steps::short()`, so the listing, the editor (`decisions.ts`), `holon mcp`'s `runs`,
  `holon decide` and the workbench read the same fields as now; step 5 replaces both with slots
  (§8.7). `StoredWaiting::new` (`:281`) takes them from `Waiting::approval`. Steps that aren't
  the short form, which a deploy refuses until step 5, count all their principals: never 0, which
  is an event's, so no token ever resumes an approval.
- `durable.rs:832` `decide`: unchanged; it counts `people` as now.
- `permissions.rs:801` `refuses_item`: refuses an `Approval(` item whose steps aren't the short
  form, as D31 says. In 4a only a hand-edited canvas can write one.
- `touches.rs:50`: the row's words from `said()`: "approved by 2 people".

### Tests in 4a

- **T1** (`approval.rs`): the canonical round trip over the universe (§T). It also has a table of
  texts that aren't canonical: `person*1`, `person:`, `agent` with no role (D33), `service:x`, six
  slots, `person*6`, a role of 65 characters, a stray space.
- `kind.rs:1308`: `Approval("payout/person*2")`. The fail-closed cases add a steps param that
  isn't canonical.
- The existing approval tests in `ts.rs`, `lower.rs` (`:4641`), `print.rs` and `durable.rs`, and
  `tests.rs`'s `approved-payout` run, take the new capability text.
- `touches.rs` and `describe.rs` word tests: "approved by 2 people", "by 2 people", and one
  long form each.
- `permissions.rs`: the short forms granted, the rest refused, and the refusal's words.
- `HOLON_BLESS=1` for `leaves.md`; `wasm-pack test --node crates/holon-wasm`; vitest
  (`decisions.test.ts` is unchanged, as the listing is).

## 4. Step 4b. Steps in the language

### `lang/ts.rs` `event()` (`:3752`)

`approval`'s options become `expires`, `people`, `steps` and `order`:

```ts
await approval("payout", { people: 2, expires: "1d" })            // the short form
await approval("payout", {
  steps: [{ role: "classifier", kind: "agent" }, { role: "reviewer" }, { role: "approver" }],
  expires: "1d",
})
await approval("payout", { steps: [{ role: "reviewer" }, { role: "approver" }], order: "any" })
await approval("payout", { steps: [[{ role: "a" }, { role: "b" }], { role: "c", people: 2 }] })
```

- `steps` is a list of slots and stages. A stage is a list of slots, and a slot is an object with
  `role`, `kind` and `people`. Each is a literal, as `expires` is.
- `order` is `"sequence"` (the default) or `"any"`, and is only taken with a flat `steps` (D32).
- Refusals, each in one sentence with the shape it wants:
  - `people` beside `steps`: "people is the short form of one step: write people in the step,
    `{ role: \"approver\", people: 2 }`";
  - a slot with no `role` whose `kind` is `"agent"`, and a `kind` other than `"person"` or
    `"agent"` ("a service never decides an approval");
  - more than 5 slots in all, or a slot's `people` outside 1 to 5;
  - `order` without `steps`, or with stages;
  - a stage of one slot (write the slot), an empty stage, and stages nested twice;
  - an unknown key, in a slot or in the options, as now.
- `ExprKind::Await`'s `approval` takes the `Steps` the options make.

### `lang/print.rs` (`:3345`)

It prints the canonical form back as the language writes it, one form for each `Steps` (D32):

- the short form prints `people: N`;
- one stage of two or more slots prints a flat `steps` and `order: "any"`;
- every stage of one slot prints a flat `steps`;
- anything else prints nested lists, a stage of one slot unwrapped.

A slot prints as `{ role: "reviewer" }`, with `kind: "agent"` when it is an agent's and
`people: N` from 2. The kind `"person"` is never written. `holon fmt` writes this form, and
`fmt` of it is itself (§T2). As built, a long one goes over lines as `fmt` wraps any options
object, and a slot with neither role nor people prints `{}`.

### Elsewhere in `holon-author`

- `editing.rs`: hover docs for `steps`, `order`, and a slot's `role`, `kind` and `people`; the
  `approval` entry (`:88`) shows a two-slot example.
- `holon.d.ts` (`src/holon.d.ts:273`): `approval(name, options?: ApprovalOptions)`, where
  `ApprovalOptions` is the short form `{ expires?, people? }` or `{ expires?, steps, order? }`;
  `Slot = { role: string; kind?: "person" | "agent"; people?: 1 | 2 | 3 | 4 | 5 }`; a stage is
  `Slot[]`. Bless the samples' copy.
- `describe.rs`: nothing more (4a's `said()` already covers every form).

### `holon`

- `bindings.rs` `RoleText::def`: a role an admin defines is held to `approval::is_role`, so
  every role can be named in an approval (D35; planned in 4a, built in 4b with the language).
- `permissions.rs` `refuses_item` (4a's rule) now has flows that reach it. Its words, in the
  preview's `refused` and the deploy's 400: `this host decides an approval by people alone for
  now ("payout" asks for an agent holding classifier, then …), and doesn't yet decide one in
  steps`. Step 5 removes the rule, in the same pull request (D31).

### Docs

- `docs/LANGUAGE.md` (approval, `:466`): the long form, its canonical text and capability, what
  weakening it asks for (§8.2), and that this host refuses it until step 5. The manual's language
  reference and tour, with the same example.

### Tests in 4b

- **T2** (`ts.rs` or `print.rs`): for each `Steps` in the universe, print → parse → lower gives
  the same canonical text, and `fmt` of the printed text is itself.
- **T3** (`ts.rs`): the refusals as one table, from the options text to the sentence.
- **T4** (`holon`, one deploy test): every two different steps in the universe are different
  items. A deployed short form whose version changes its `people` asks for the new item, and an
  agent's such version is a proposal (item 19 holds already).
- `permissions.rs`: a table of steps texts, each refused or not by `refuses_item`.
- `samples.rs`' fmt test, and `samples.test.ts`, unchanged: no sample writes steps yet.

## 5. Step 4c. The decision as records

D30 keeps `by` and adds what a slot needs:

```ts
interface Decision {
  approved: boolean
  by: string[]              // who approved, by name, in the order they did (as now)
  decisions: Decided[]      // every decision, a decline included, in the order made
  declined_by?: string
  reason?: string
  expired?: true
}
interface Decided {
  slot: number              // the slot's number, as the steps are written (0 for the short form)
  who: string
  kind: "person" | "agent"
  role?: string             // the slot's role; none for the short form
  at: number                // seconds since the epoch
  approved: boolean
  reason?: string
  note?: unknown            // from step 5 (§8.3)
}
```

- `durable.rs:287` `StoredWaiting::decision` builds `decisions` in place of `approvals`. `decide`
  keeps the decline among them: `Approved` (`:267`) becomes `Decided`, with `approved: bool` and
  the slot, kind and role. Each counts as now.
- `holon.d.ts`: `Decision` and `Decided`.
- `approved-payout` reads `decision.by` and `decision.reason`, which stay, so its text doesn't
  change. Its run's answer, in the e2e and live, shows `decisions`.
- `docs/LANGUAGE.md`, the manual's reference: the value.

As built, the waiting point keeps the same records, `decisions` in place of `approved`, and the
value is made from them (`StoredWaiting::take`), so a decline a run lock turned away is still the
decision when someone decides again. The listing's readers (`decisions.ts`, `holon decide`, the
`runs` tool) count the approvals among them. The decider's kind is the caller's, a person until
step 5.

### Tests in 4c

- **T5** (`durable.rs`): one table of approve, approve; decline; expire. Each row gives the value
  the run receives, `decisions` and all.
- `tests.rs` (`approved-payout` and the durable tests) and `mcp.rs` (`explain` shows the value):
  the new field.

## 6. Where step 4 leaves step 5

- `StoredWaiting` has `steps`, the canonical text, beside `people`, which only the short form
  uses, and `decisions`, each with its slot (4c). Step 5 replaces `people` with the slots, which
  `decisions` says who filled, and which are open (§8.7), and lists them so.
- Deciding by slot, the slot's role at a scope covering the flow (bindings, step 3), the kind,
  `one principal, one slot`, `not the starter`, `exclude_deployer`, `approvals.agents` (§8.3,
  §8.4), `note` (16 KiB), `holon decide --slot --note`, and the MCP `decide` tool for agent
  slots.
- `ParkedRun` keeps who started the run, for "not the starter".
- `permissions.rs`'s refusal of steps goes, and `approved-payout` moves to the chain: an agent
  classifies, an editor reviews, an admin approves. It is run live with three principals (§13).

## 7. Step 5. The server decides by slot

What a flow can write since 4b, the host now decides: each slot by a principal of its kind holding
its role, in its order, under the design's fixed rules (§8.3, §8.4). Five commits, each passing
every check. Lines are at `661b9a16` (4c).

| Sub-step | Change | Where | Decisions |
|---|---|---|---|
| 5a | Deciding by slot: a point's slots, open or filled; the slot named or picked; the role, the kind, one principal one slot, not the starter; the note; the answer and the listing; `permissions.rs`'s refusal goes | `holon` (`durable.rs`, `can.rs`, `listeners.rs`, `deploy.rs`, `permissions.rs`, `decidecmd.rs`, `mcp.rs`), the editor (`decisions.ts`, `ApprovalsPage.tsx`, `server.ts`) | D36 to D40, D44 |
| 5b | The host's approval settings, `exclude_deployer` and `agents` | `holon` (`bindings.rs`, `permissions.rs`, `accesscmd.rs`) | D41 |
| 5c | `holon decide --slot --note`, with an agent's token too; the MCP `decide` tool for agent slots | `holon` (`decidecmd.rs`, `mcp.rs`) | D42 |
| 5d | The evidence: each decision with the binding and access version that let it; `approved-payout` on the chain, run live with three principals | `holon` (`durable.rs`), the sample, docs | D43 |
| 5e | The run file signs in as the account that set the server up: one principal, its bindings, on the record | `holon` (`access.rs`, `accounts.rs`, `bindings.rs`, `can.rs`, `durable.rs`, `activity.rs`, `auth.rs`, `accesscmd.rs`, `mcphosts.rs`), the sample, docs | D45 |

### Step 5a. Deciding by slot

**The waiting point** (`durable.rs:255` `StoredWaiting`). `people` goes. The slots are the
steps' (`Steps::parse(&steps)`), and `decisions` says who filled each. In place of `approvals`,
`approved` and `take`:

- `slots()`: each slot with its number, its stage, role, kind and `people`, who approved it, and
  whether it is open: every earlier stage filled, and itself not;
- `take(d)`: refuses a decision on a slot that isn't open, and one by a principal who already
  decided a slot of this point (one principal, one slot, and once within it, §8.4). The decision
  to deliver once every slot is filled, or at the first decline;
- an event's point has no steps: the `people == 0` tests (`:547`, `:853`, `:908`) read
  `steps.is_empty()`.

**Picking the slot** (D36). `decide` (`:888`) takes `slot: Option<u8>`. Without it, it looks at
the open slots the caller may fill: one is taken; none is 403, naming the open slots and what each
needs; two or more (in any order) is 409, asking for `slot`. With it, the slot must exist and be
open: 409, "slot 2, approver, opens once reviewer is filled".

**Whether the caller may fill it** (D37, D38, D39), under the lock, from the caller's `holds`
(the `Held` list `can` reads, `bindings.rs:204`):

- `can(caller, run.decide, the point)`, as now (`decider`, `:941`);
- a slot with a role: some binding of that role, by name, directly or through a group, covers
  the point (`Held::covers`, `bindings.rs:236`). Holding everything (admin) isn't holding
  `reviewer`. A slot without a role takes anyone `run.decide` lets in, as now (D33);
- the kind: a person's slot takes a person, an agent's an agent, and a service none. `can.rs:357`'s
  fixed rule stops refusing an agent `run.decide` outright, and the slot refuses it a person's
  slot: "claude is an agent: slot 1, reviewer, is a person's";
- not the starter: `ParkedRun` (`:382`) gains `started_by: Option<String>`, the principal on the
  run's first part: a control-plane call's `by` (`listeners.rs:304`) or the action's caller
  (`deploy.rs:1122`), passed to `park_if_waiting` (`:664`); the other triggers pass none.

**The note** (D40). `DecideBody` (`:927`) takes `slot` and `note`: any JSON, at most 16 KiB as
written, a person's or an agent's. It is kept on `Decided`, and given to the run as its `note`
(`Decided::value`, `:292`), read as a run reads a JSON body.

**The answer and the listing** (D44). The decide route answers
`{name, slots: [{slot, stage, role?, kind, people, by, open}], outcome}`. `ParkedRun::summary`
(`:647`) lists an approval point so, with `said` (`Steps::said`) and `decisions`; an event's
point stays `{name, expires_at}`. Each approval point also lists `yours`, the open steps the
caller may fill (the decide route's checks, without deciding), so the editor offers those and the
nav counts them without asking `can` per point. `people` and `approved_by` go from both (dev
phase), and their readers follow:

- `decidecmd.rs:114` `decided_said` and `:136` `waiting_lines`: the slots in words, "reviewer:
  ada approved; approver: open";
- `mcp.rs:891` `runs`, the same lines;
- the editor: `decisions.ts:12` `isApproval` reads `steps`, and `:34` says the chain in words.
  `ApprovalsPage.tsx` sends the slot it offers, named on its buttons when the caller can fill
  two, and says "you approved as reviewer" where they did. The chain drawn as a row is step 7's.

**The refusal goes.** `permissions.rs:824` and its test's rows (`:1271`): every canonical steps
text deploys.

**Tests in 5a**

- **T6** (`durable.rs`): one table over a cast (two people holding reviewer, one holding approver,
  an agent holding classifier, an admin, the run's starter) and steps of each shape (sequence,
  any order, stages, a slot of two). Each row is decisions in turn, each taken or refused with
  its sentence, and the value at the end. It covers the order, the role, the kind, one principal
  one slot, the starter, a decline ending it, and an expiry with the decisions so far.
- `can.rs`'s route table: `run.decide` for an agent holding a role with it, at the point.
- `tests.rs`, the route: a slot named, picked, ambiguous; a note too long; an agent on a person's
  slot (403).
- `decidecmd.rs`, `mcp.rs` and `decisions.test.ts`: the new lines.

### Step 5b. The host's approval settings

§8.4's two settings go in the access document (§6), beside the roles and bindings (D41):
`approvals: { exclude_deployer, agents }` on `Document` (`bindings.rs:41`), `false` and `true`
by default. A change makes a version, with who, when and why, as a binding does.

- `exclude_deployer`: whoever deployed the version the run is on (`ParkedRun.version`, the
  deployment's version record) fills no slot.
- `agents: false`: a deploy whose manifest has an `Approval(…)` with an agent's slot is refused
  (`ApprovalSettings::refuses`, beside the policy's refusals in `deploy.rs`, the preview's
  `refused` too), and an agent fills no slot of a run already waiting.
- Set with `PUT /access/approvals` (`people.assign`) and
  `holon access approvals [--exclude-deployer on|off] [--agents on|off] [--why …]`
  (`accesscmd.rs:337`); `holon access` shows them.
- Tests: one table of the two settings against a deploy and a decision.

### Step 5c. `holon decide`, and the MCP `decide` tool

- `holon decide SLUG RUN NAME --approve|--decline [--reason …] [--slot N] [--note JSON]`
  (`decidecmd.rs:10`). With an agent's token it fills an agent's slot; a person's slot answers
  the route's 403.
- `holon mcp` gains `decide` (D42), `{slug, run, name, approve, reason?, note?, slot?}`, as the
  agent signs in. It fills an agent's slot when the agent holds its role. For a person's slot it
  decides nothing, and says who decides it and where (the editor's Approvals, `holon decide`).
  The instructions (`mcp.rs:59`) and `runs` (`:265`, `:908`) say an agent decides only a slot
  written for an agent. `runs` and `holon decide <slug>` list the caller's open steps, with the
  number `--slot` takes (`decidecmd::yours_lines`). A caller bound to no role holding
  `run.decide` where the approval is is told so (`can.rs`), not that deciding is a person's.
- Tests: `mcp.rs`, one table of agent and person slots, open or not, through the tool.

### Step 5d. The evidence, and the sample

- **The binding** (D43). Each `Decided` keeps what let it: the binding's id and the access
  version in force (`Bindings::document().version`), or `local` for the OS user's admin. The run
  is given them with the decision (`binding`, `access`), so the resumed part's log entry carries
  them, and the export and `holon verify` read them as logged input. An auditor checks the order,
  the kinds, the different principals and their bindings offline, against `holon access show N`.
- **The timeline.** The activity log says each decision with its slot ("ada reviewed payout for
  approved-payout run 7b37…, slot 1"); the resumed part's entry carries them all (4c).
- **`approved-payout`** moves to the chain, tagged `payments`:
  `steps: [{ role: "classifier", kind: "agent" }, { role: "reviewer" }, { role: "approver" }]`.
  Its comment says what to bind; its try-it classifies with `holon decide --note` as an agent,
  and reviews and approves as two people. `samples.rs`'s entry and the goldens follow.
- **Run live**, two people, an admin and an agent on scratch 7530 (the flow on 8230): out of turn
  refused; one principal refused a second slot; the starter (who ran the action) refused; then
  paid. A second run declined by the reviewer with a reason. The export holds each decision's
  binding.
- **e2e**: `approved-payout` decided through the real server by an agent's token and two people.
- **Docs**: `docs/LANGUAGE.md` and the manual's approvals pages (deciding by slot, the fixed
  rules, the note); `holon.d.ts` (`Decided.note`, `binding`, `access`); the server reference (the
  route's body and answer, the listing); the MCP instructions; the CHANGELOG.

### Step 5e. The run file is the account that set the server up

Found running 5d live: the run file (local.rs) signs in as `local:<OS user>`, holding admin on
the host and nothing bound. So it takes no step with a role, even when the human at the keyboard
is bound to it; beside that human's own account it counts as a second person (`people: 2` was
approved "by BMA and ada"); and a run it starts, or a version it deploys, is recorded under the
OS user's name, which the starter rule and `exclude_deployer` don't match to the account.

- **The tie** (D45). The access document keeps `local`, the account the run file signs in as.
  Setup sets it to the account it makes, in the same version: whoever entered the setup code saw
  what the server printed to the OS user who runs it, as the run file is that user's. Removing
  that account clears it, in the same version. `PUT /access/local` (`{account, why}`,
  `people.assign` on the host) or `holon access local --as NAME` moves it to an enabled person,
  for a handover; `holon access show` says it.
- **The gate** (`access.rs` `authenticate`). The run file's token, tied to an enabled person, is
  that person: their name, holding what is bound to them, then admin on the host, so the run file
  still operates the host whatever the account holds. `Caller.local` keeps the OS user. Untied
  (before setup, or its account disabled) it is `local:<user>`, admin alone, as before.
- **Deciding.** Only through a binding of the document: `can::holding` skips what no document
  holds, so every decision names a binding, and `"local"` goes from `binding` (`Through` goes;
  `binding` is the id). The run file tied to no account decides nothing (`durable::decider`),
  saying so. The `local:` stripping that made the OS user and an account of its name one person
  goes (`as_decider`, `decider`).
- **One name everywhere.** The run's starter, the version's deployer, `created_by` and an owner
  are the account. Agents the OS user owns before setup (`holon mcp`'s `<user>-agent`) move to
  the account at setup. `/auth/me` answers the account, with `local`, the OS user, and
  `holon mcp setup` names the agent from it.
- **The channel.** An activity entry made through the run file says so, `via: "local:<user>"`
  (`activity.rs`, from the request being served): on a host where someone other than the setup
  person has the run file's shell, the record says how the name signed in.
- **`approved-payout`**: the approver is you, `holon bind approver --to <you> --tag payments`,
  approving with `holon decide` on the server's machine.
- **Tests**: one table, the run file and a session against a role step bound to the account,
  `people: 2` by the same human, the starter, the deployer under `exclude_deployer`, and the run
  file untied; accounts.rs: the tie set at setup, cleared at removal, moved; the access table's
  run-file row; `holon mcp setup`.
- **Run live**: set up as ada; `holon decide` through the run file takes the approver's step,
  its binding ada's; `people: 2`, the run file then ada's session: once; a run ada starts from
  the CLI refused to her in the editor; a version she deploys from the CLI under
  `exclude_deployer`; the activity entry's `via`; `holon access local --as bob`; ada disabled:
  the run file decides nothing.
- **Docs**: the people guide, the cli and server references, `docs/LANGUAGE.md` and the manual
  (`binding` an id), `holon.d.ts`, the CHANGELOG.

### Checks for step 5

| Sub-step | cargo test, clippy | wasm-pack | tsc, vitest | `npm run e2e` (slot 3) | Run live |
|---|---|---|---|---|---|
| 5a | yes | | yes | yes | a chain decided by slot, each refusal said |
| 5b | yes | | | yes | each setting against a deploy and a decision |
| 5c | yes | | | yes | an agent's slot through `holon decide` and the tool |
| 5d | yes | yes (the sample) | yes | yes | §13's run, and its export |
| 5e | yes | yes (the sample) | yes | yes | the run file and a session, one principal; the tie moved and broken |

## T. The universe of steps

One generator, `holon_core::approval::universe()` (`#[doc(hidden)] pub`, since `#[cfg(test)]`
code isn't seen from another crate's tests), used by T1, T2 and T4:

- the slot variants: a person with no role, or with `reviewer` or `x-1`; an agent with `reviewer`
  or `x-1`; each with `people` 1, 2 or 5 (fifteen);
- every slot variant alone;
- every shape of stages, one to three stages of one to three slots and at most five slots in all,
  filled by going round the variants from each starting point.

That is a few hundred `Steps` (21 shapes from 15 starts, and the 15 alone), and the round trips run
over all of them in well under a second. Every variant meets every position, and every shape is
there; every combination isn't, which would be millions.

## D. Decisions

Settled 2026-11-12, each as recommended: D29 to D35, and step 5's, D36 to D44.

- **D29. The canonical text and the capability.** The design writes
  `Approval("payout", "seq[agent:classifier, person:reviewer, person:approver]")`, two arguments.
  A capability holds one text (`kind.rs:142`), and a manifest item names one argument.
  - **Recommended**: one text, `name/steps`, with §2's grammar:
    `Approval("payout/agent:classifier>person:reviewer>person:approver")`. The short form is
    `person*N`, with the kind always written, so there is one form for every approval.
  - Otherwise the design's `seq[…]`/`any[…]` words inside the one text. They are longer, and
    `seq` is only a stage of one slot, said again.
- **D30. The value the run receives.** The design makes `by` a list of `Decision` records and
  adds `declined: Decision`.
  - **Recommended**: keep `by` (names) and `declined_by`, which `approved-payout` and the docs
    read, and add `decisions: Decided[]` in place of today's `approvals`. Every decision is in
    `decisions` with its slot, kind and role, a decline included.
  - Otherwise follow the design. Every flow that reads `decision.by` then reads records, and the
    sample changes to `decision.by.map((d) => d.who)`.
- **D31. Between steps 4 and 5.** From 4b a flow can write steps that the host can't yet decide
  by role.
  - **Recommended**: one branch and one pull request for steps 4 and 5. Until step 5's commit the
    host refuses, at deploy, any approval but the short form (`permissions.rs`). No commit has a
    host that lets anyone holding `run.decide` fill a slot that names a role.
  - Otherwise a pull request each, with the refusal released in between.
- **D32. Two ways to say "any order".** `order: "any"` with a flat list, and a stage written
  `[[a, b, c]]`, are the same steps.
  - **Recommended**: take both, as the design does. `order: "any"` reads plainly, and stages need
    nesting anyway. `fmt` prints one form for each (§4), and `order` with stages is refused.
  - Otherwise nested lists only, and `order` goes.
- **D33. Slots without a role.**
  - **Recommended**: a slot with no `role` is a person's, filled by anyone holding `run.decide`
    here. That is the short form, and the step 3 plan's answer to the design's §15 Q2. An agent
    slot always names its role: an agent decides only where a role was given to it on purpose.
    A service never fills one.
  - Otherwise an agent slot without a role is any agent holding `run.decide`. The built-in agent
    role doesn't hold it, so only a custom role could give it.
- **D34. Agent notes, the design's §15 Q3.**
  - **Recommended**: no note-only slot in step 4. An agent slot approves or declines, and a note
    rides with either (step 5, §8.3). A classifier that never blocks is a slot of its own kind
    later, if asked for: a new field in the slot, and so a new capability.
  - Otherwise step 4 adds `decides: false` to an agent's slot now, and step 5 takes a note there
    without an approval.
- **D35. Role names in the steps.** A role's name is any slug (`deploy.rs:51` `is_valid_slug`, no
  length cap); the steps take `ident`'s rule (at most 64).
  - **Recommended**: hold the roles an admin defines to the same 64 (`bindings.rs`, `PUT
    /roles`), so every role can be named in an approval. The built-in ones are short already.
  - Otherwise a longer role can't be named in an approval, and `holon check` says so.

- **D36. When a decision names no slot.**
  - **Recommended**: the design's (§8.3). The one open slot the caller can fill is taken; none is
    403, naming the open slots and what each needs; two (in any order) is 409, asking for `slot`.
  - Otherwise the first open slot in writing order, always.
- **D37. Holding a slot's role.**
  - **Recommended**: by name. The caller is bound to that role, directly or through a group, at a
    scope covering the approval point, and holds `run.decide` there. An admin fills a `reviewer`
    slot only when bound to reviewer: deciding is granted on purpose. A built-in role's name is a
    role too (`{ role: "admin" }`).
  - Otherwise any principal whose roles there hold everything the named role holds, so an admin
    fills every slot (§8.4's "admin holds everything").
- **D38. Agents and `run.decide`.**
  - **Recommended**: `can.rs`'s fixed rule lets an agent decide, and the slot's kind refuses it a
    person's slot. A service decides nothing.
  - Otherwise keep the fixed rule, and let an agent through inside `decide` for an agent's slot
    only.
- **D39. The run's starter.**
  - **Recommended**: the principal on the run's first part: a control-plane call's `by`, or the
    action's caller. A request on the flow's own port names nobody, and a clock, a file, a queue,
    a mail or a feed starts a run with no starter.
  - Otherwise no starter rule until the flow's own port signs callers in.
- **D40. The note.**
  - **Recommended**: any JSON up to 16 KiB, a person's or an agent's, given to the run with the
    decision (`note`) and recorded as input, like an event's body.
  - Otherwise an agent's only.
- **D41. Where the approval settings live.**
  - **Recommended**: the access document (§6), versioned with the roles and bindings, changed by
    whoever holds `people.assign`.
  - Otherwise the host's policy (`holon.toml`, beside `allow_plain_http`), changed by editing the
    file.
- **D42. The MCP `decide` tool.**
  - **Recommended**: one tool, for agent slots only. For a person's slot it decides nothing, and
    says who decides and where.
  - Otherwise no tool: an agent decides through `holon decide` with its token.
- **D43. The binding on the record.**
  - **Recommended**: the binding's id and the access version, given to the run with each decision,
    so the log and the export carry them.
  - Otherwise kept on the parked run and in the activity log, out of the run's value.
- **D44. The listing's shape.**
  - **Recommended**: `people` and `approved_by` go. A point lists `slots`
    (`{slot, stage, role, kind, people, by, open}`), `said` and `decisions`, and every reader
    follows in 5a.
  - Otherwise keep `people` and `approved_by` beside `slots`, for the short form.
- **D45. The run file's account.**
  - **Chosen**: tied to the account that set the server up, at setup; moved by an admin
    (`holon access local --as`), cleared when the account is removed; each entry it makes says
    `via` the run file.
  - Otherwise an explicit link an admin makes (friction for a one-person server), or a run file
    that decides no approval (a person then has no command line to decide from).

The design's §15 Q2 (the short form) and Q4 (editors keep `run.decide`) were settled in step 3's
plan, and step 4 builds on them. Q5 (deciding from a signed link) is later.
