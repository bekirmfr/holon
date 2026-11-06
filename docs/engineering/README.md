# Holon engineering docs

What Holon is made of, why it is made that way, how far each part reaches and where it stops,
and what the tests hold it to. Written for someone who will change it.

Holon's other docs answer other questions:

| Where | Answers | Kept |
|---|---|---|
| [`docs/manual/`](../manual/README.md) | How do I use it? | for users, as the product is |
| [`docs/LANGUAGE.md`](../LANGUAGE.md) | What does the language say? | the language's reference |
| [`docs/L4s/`](../L4s/) | What did we decide for this change, and what did we try? | one note per piece of work, as it was done; not rewritten after. Host capabilities are designed in one, [host capabilities](../L4s/host-capabilities-L4.md), a section each |
| `docs/engineering/` (here) | Why is it this way now, and what holds it there? | rewritten as the code changes |

An L4 note is a record of the work: its plan, measurements and reversals in the order they
happened. A doc here is the current state distilled from those notes, linking back to the § that
holds the evidence for a claim. When the two disagree, the code is right, then this folder, then
the note.

## The docs

**Themes**, across the codebase:

- [Concepts](themes/concepts.md): what a person changing Holon must hold, each pointing where it
  lives and the doc that goes deeper. Start here.
- [Performance](themes/performance.md): what a request goes through, what it costs, and why each
  part is the way it is.
- [Resilience](themes/resilience.md): what the server guarantees under load and failure, how each
  guarantee is kept, and its limits.
- [Security](themes/security.md): who may do what, what a flow can reach, what is kept secret
  and how, and what isn't defended.
- [Testing](themes/testing.md): which tests hold which property, and why each exists.
- [Measuring](themes/measuring.md): how a cost is measured here, and the traps.
- [Lessons](themes/lessons.md): what the work taught, the designs that were worse included.

**Components**, one per crate or part:

- [holon-core](components/holon-core.md): units, values, types, the evaluator and its plan, the
  primitives, hashing.
- [holon-host](components/holon-host.md): the store and its files, writing aside, packs, the
  registry.
- [holon](components/holon.md): the server and the CLI: control and data planes, deployments,
  the run path, its threads and locks.
- [holon-author](components/holon-author.md): the language, its compiler and printer, `holon fmt`,
  the samples, the golden records, the language server.
- [holon-wasm](components/holon-wasm.md): the editor's bridge to the core, and how it is built.
- [holon-module](components/holon-module.md): wasm modules, and the js engine they carry.
- [The editor](components/editor.md): the web editor, its tests, and the terminal editor beside it.

**Capabilities**, one per host capability:

- [State and collections](capabilities/state-and-collections.md)
- [Outbound requests](capabilities/outbound-requests.md)
- [Secrets](capabilities/secrets.md)
- [Folders](capabilities/folders.md)
- [Databases](capabilities/databases.md)
- [Durable runs](capabilities/durable-runs.md)
- [Triggers and clocks](capabilities/triggers-and-clocks.md)
- [Run records and `holon verify`](capabilities/run-records-and-verify.md)
- [The js node](capabilities/js-node.md)
- [Storage](capabilities/storage.md): sweeping, packs, sealed logs, a full disk, the backup, the
  host's view.

The git history here starts at the 0.1.0-rc.29 release commit (`6bc5c68`); what came before is
given by its CHANGELOG release, or by date from the [journal](../JOURNAL.md) and the L4 notes.

## How a doc here is written

Each follows the same outline, leaving out a section with nothing in it:

1. **What it is**: a paragraph, and where it lives in the code.
2. **How it works**: the mechanism, named as the code names it (functions, constants), so a reader
   can find it.
3. **Why it is this way**: each decision, the reason for it, and what else was weighed; a link to
   the L4 § that holds the evidence.
4. **Reach and limits**: what it does, how far (with numbers where measured), and what it does not
   do or does badly. A limit is written down, not left to be found.
5. **What the tests hold**: the tests that guard it, what each asserts, and the gaps.
6. **History**: the changes that made it what it is, by commit, newest last.

Numbers are given with their conditions (route, connections, machine, build) and the commit they
were measured at; a number measured once is said to be. A doc changes in the same commit as the
code it describes.

## Standards these docs record

These run through the themes, and are kept by every change:

- **Resilience before throughput.** A fix that makes the server safer is taken even when it costs
  speed, and its cost is measured and written down. A speed-up that weakens a guarantee is not
  taken, or is made a choice a person makes (a handler that says `lazy` or `unrecorded`).
- **Every cost measured, both ways.** A change made for speed shows what it gained; a change made
  for safety shows what it cost. Measured as [Measuring](themes/measuring.md) says, not estimated.
- **Nothing changes by accident.** Golden records hold the evaluator's answers, records and hashes;
  a faster path is tested against the plain one it stands in for.
- **One source of truth.** What must agree is written once (the primitives' list, the counting
  rule), or checked against the other copy where it can't be.
- **Fail closed, and loudly.** A case the code doesn't expect answers nothing, or an error, never a
  guess; a silent fallback to a slower path has a test that trips when it is taken.
- **Every feature ships with a sample flow**, run live, deployed by the end-to-end tests
  ([`CLAUDE.md`](../../CLAUDE.md)).
