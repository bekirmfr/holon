# Holon engineering docs

What Holon is made of, why it is made that way, how far each part reaches and where it stops,
and what the tests hold it to. Written for someone who will change it.

Holon's other docs answer other questions:

| Where | Answers | Kept |
|---|---|---|
| [`docs/manual/`](../manual/README.md) | How do I use it? | for users, as the product is |
| [`docs/LANGUAGE.md`](../LANGUAGE.md) | What does the language say? | the language's reference |
| [`docs/L4s/`](../L4s/) | What did we decide for this change, and what did we try? | one note per piece of work, as it was done; not rewritten after |
| `docs/engineering/` (here) | Why is it this way now, and what holds it there? | rewritten as the code changes |

An L4 note is a record of the work: its plan, measurements and reversals in the order they
happened. A doc here is the current state distilled from those notes, linking back to the § that
holds the evidence for a claim. When the two disagree, the code is right, then this folder, then
the note.

## The docs

**Themes**, across the codebase:

- [Performance](themes/performance.md): what a request goes through, what it costs, and why each
  part is the way it is.
- [Resilience](themes/resilience.md): what the server guarantees under load and failure, how each
  guarantee is kept, and its limits.
- [Testing](themes/testing.md): which tests hold which property, and why each exists.
- [Measuring](themes/measuring.md): how a cost is measured here, and the traps.
- [Lessons](themes/lessons.md): what the work taught, the designs that were worse included.
- Still to write: concepts, security.

**Components**, one per crate or part (to write): holon-core, holon-host, holon (the server),
holon-author (the language, compiler, `fmt` and samples), holon-wasm, holon-module, the editor.

**Capabilities**, one per host capability (to write): state and collections, outbound requests,
secrets, folders, databases, durable runs, triggers and clocks, run records and `holon verify`, the
js node.

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
