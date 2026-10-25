# The code node: `js` on the module unit — Level-4 design

> *crate:* **`holon-author`** (the `js` statement: parse, lower, print) **+ `holon-module`** (the
> engine module's build and its ABI conformance tests) **+ `holon-core`** (nothing beyond what
> `wasm-module-L4.md` already pins) · *pre:* `wasm-module-L4.md` inches 1 to 3 (the module body,
> the engine hook, the host and bridge wiring) · *inv:* `I1` / `I2` / `I5` / `I6` / `I7` / `I8` ·
> *effort:* **M** (three inches after the module unit exists, about a week)
> *Status:* **built 2026-10-25** (the engine module, the statement, the sample `nearest`); what
> §7 still owes is listed there. The trigger in `wasm-module-L4.md` §7 was met the same day (§0).
> *Pinned:* a `js` node is a **composite over one pinned engine module and its source as a
> `const`**, never a new body kind · the engine is a module like any other, **no imports** · the
> engine's realm is **fresh per call**, with **no clock, no randomness, no I/O** · source crosses
> as text, values cross as core's value encoding · the text **names no engine hash**.

---

## 0. What this is, and why now

`wasm-module-L4.md` designed the vocabulary's escape hatch, a WebAssembly module with no imports
as a third unit body, and said it would be built when a person or an agent hit the ceiling of the
seventy leaves twice, with neither case a candidate for one small pure leaf. That happened on
2026-10-25, in one session, writing flows through `holon mcp`:

1. **An interpreter.** A stack machine for a Forth-like language, with a dictionary in a
   collection, was written in the vocabulary: `fold` over the words with the machine as the
   accumulator, records as dispatch tables, text join and split as macro expansion. It worked,
   and it ran into the language's shape at every turn: nested definitions needed a fixed fuel of
   expansion passes, every operation had to be made total by hand so one absent candidate would
   not empty the dispatch table, and the dispatch had to be split across holons to stay
   readable. Not one of those is a missing leaf. All of them are "this is code".
2. **A game's rules.** Chess by correspondence is a natural flow: a game in a collection, a
   signed token for whose turn it is, a durable record of every move. Its one dense part, "is
   this move legal and what is the position after it", is unwritable in the vocabulary at any
   size, because check detection is a search.

Both are the second case of §7's first trigger, and neither wants a leaf. So the module unit is
built as designed, and this note designs the layer that makes it usable by the people who will
actually reach for it: a statement that takes **source in a language they know** and makes it
one node, with every guarantee of the module unit and none of its build step. JavaScript first,
because it is the one most authors know, it needs no toolchain on anyone's machine, and the
source stays in the `.holon` text where `fmt`, review and the editor already are. Other languages
are §8: a sentence each, because they add reach and not power.

This note decides the `js` statement, its lowering, the engine module's contract, how values and
errors cross, budgets, the manifest, `explain`, the editor, and the sample. It changes nothing in
core beyond `wasm-module-L4.md`.

## 1. One primitive, many languages (I1)

Core knows one new thing, `UnitBody::Module { bytes }`. A "language node" is one of two ways of
getting to a module, and neither is a core concept:

| Language kind | What the node is | Source lives | Toolchain | Example |
|---|---|---|---|---|
| Interpreted (JavaScript, later Lua, Python…) | a composite: the language's **engine**, itself a module unit the project ships, plus the source as a `const`, plus the glue that shapes ports | in the flow's text | none | `js` (this note) |
| Compiled (Rust, C#, Zig…) | the module unit itself, built elsewhere, placed by hash | beside the flow or in a repository, with a build recipe | the language's, run by `holon build` or CI | `module` (`wasm-module-L4.md` §6), `rust` (§8) |

Everything that makes a module safe, the empty import section, the ABI, fuel and memory, the
canonical value encoding at the boundary, replay by re-running the same bytes, belongs to the
module unit and is inherited unchanged. A language node adds a surface, never a guarantee, and
never a hole in one.

## 2. The statement

```
js score = (estimates, tags) -> (total, weight) {
  const total = estimates.reduce((a, b) => a + b, 0);
  return { total, weight: total > 10 ? "heavy" : "light" };
}

js nearest = (word: Text, candidates: List) -> Text fuel 50M {
  …
}
```

- Top level only, beside `holon`, visible to the whole file in any order. Placed by name like a
  holon: `s = score(req.json.estimates ?? [], tags)`, `score(…).weight`.
- Ports as a holon declares them: names, optional types, one output or a parenthesised list.
  Port names are the JavaScript parameter names and, for several outputs, the keys of the object
  the body returns. A single output is the value the body returns.
- `fuel N` and `memory N` after the ports, optional. The defaults are a js node's own, measured
  (`holon-module/tests/js_engine.rs`, `what_a_call_costs`): a fresh realm and a compiled one-line
  function cost about 15M instructions under the interpreter, an edit-distance search over eight
  words 41M, a loop of a hundred thousand 271M; so a js node defaults to **200M** fuel
  (`JS_DEFAULT_FUEL`), not the module's 10M, which could never run a body, and to the module's
  16 MiB. `K`, `M`, `G` scale a number (`50M`), `KB`, `MB`, `GB` a size (`16MB`); `fmt` writes
  the number out.
- The body is JavaScript source between the braces, taken verbatim: the lexer finds the matching
  close brace by counting braces outside JavaScript strings, template literals and comments, so
  a `}` in a string does not end the node. Nothing inside is parsed as Holon; nothing inside is
  interpolated. A `js` body is the one place in the language where the text's content is not
  structure (P3 does not apply inside it: the source is identity, §3).
- Grammar, added to §5.2 of `docs/LANGUAGE.md`:

```
jsnode    = "js" ident "=" "(" ports? ")" "->" outputs budget* jsbody
budget    = "fuel" number | "memory" number
jsbody    = "{" <JavaScript source, balanced> "}"
```

## 3. Lowering: a composite the printer can read back

`js score = (a, b) -> (total, weight) { body }` lowers to one composite:

```
inputs:   a, b                                   (the declared ports, declared types)
children: const{value: "<function source>"}      the source, as text
          list(a, b)                             the arguments, in declared order
          engine<fuel, memory>(source, args)     the pinned engine module, §4
          field{total}(result.value)             one per declared output
          field{weight}(result.value)
outputs:  total, weight
```

- **The function source** the `const` holds is `(a, b) => { body }`, assembled by the lowering
  from the declared names and the body: the engine receives one text and compiles it as a
  function, so the ports and the source cannot disagree. A single output lowers without the
  `field` nodes: the output is `result.value` itself.
- **The engine** is a child by hash. The catalog's engine entry (`holon-module` exposes it to
  `holon-author` the way the primitive catalog is exposed) gives the hash of the engine module
  under the **default** budgets. A declared budget instantiates the engine template (its `fuel`
  and `memory` are `Param::Open`, `promote.rs`) with those values, which is a new concrete unit
  with its own hash, inserted into the store like any instance. So a budget is part of the
  node's identity, as `wasm-module-L4.md` §3.5 requires, through machinery that exists.
- **Shape is identity.** Two `js` nodes with the same ports, source and budgets are one unit, in
  any flow. A change to a character of the body is a new unit. Binding and node names are not in
  it (P3); the body's text is, verbatim, including whitespace and comments, because the engine
  sees it. `fmt` therefore never touches a body. The one normalization is the line ending: a
  body is stored with `\n`.
- **The printer** recognises the shape (a composite whose children are exactly one `const`, one
  `list`, one engine instance and zero or more `field` nodes wired as above) and prints the
  `js` statement back, taking the parameter names from the source's parameter list and the
  output names from the `field` params. A composite that is almost that shape prints in the
  wiring form, as §11 of the language already does for anything it cannot express.
- **The text names no engine.** A `.holon` file says `js`, not `js@<engine hash>`. The flow
  document carries the engine's hash in its `units` closure, so a deployed version is exactly
  pinned (I6), and a file re-lowered after an engine upgrade produces new unit hashes: a new
  version, with nothing new to approve, because the manifest is unchanged (§6). This is the same
  trade the language made for leaves (the IR is the program, the text is a way to write it), and
  it keeps old files readable by new tools. An author who must pin the engine places the module
  by hash with the `module` statement instead.

## 4. The engine module

A module unit the project builds and ships, satisfying `wasm-module-L4.md` §3 to the letter:
empty import section, the `memory` / `alloc` / `run` exports, values in core's encoding. Its
interface:

```
engine<fuel, memory>(source: Text, args: List<Any>) -> (result: Record)
```

**What it does per call.** Creates a fresh JavaScript realm, installs the prelude, compiles
`source` as a function expression, applies it to `args` spread as arguments (each decoded from
core's value, §5), and answers `{ value: <returned> }` or `{ error: <message> }`. Then the realm
is gone. Nothing survives between calls: no globals, no module cache, no state. That is what
makes two calls with equal inputs equal (I2), and it is not negotiable for speed; instance
*compilation* may be cached by module hash, realm *state* may not.

**The prelude** is the determinism rule made concrete. Before the source is compiled, the realm
has:

- no `Date` (the constructor and `Date.now` are removed; a flow that wants the time wires `now`
  in as an argument, as every leaf does);
- `Math.random` removed (wire `random` in);
- no `setTimeout`, `setInterval`, `queueMicrotask`, `Promise` resolution across the call, no
  `fetch`, `XMLHttpRequest`, `WebAssembly`, `eval`'s host hooks, `import()`; the function is
  synchronous and its return value is the result. An `async` function returns a Promise, which
  is not a value (§5), so it is an error: "a js node is synchronous";
- no `Intl`, and `toLocaleString` is the plain `toString`: locale is environment;
- strict mode, and the standard library otherwise as the engine ships it: `JSON`, `Math` (less
  `random`), `Array`, `String`, `Map`, `Set`, `RegExp`, `BigInt` (which cannot cross, §5).

Each removal is a test in `holon-module`: a source that reaches for the name gets an error naming
it, and the test suite is the list of what a `js` body cannot do.

**Which engine.** The contract above is the design; the engine is an inch decision, and the
2026-10-25 search for one that builds to `wasm32-unknown-unknown` with **no imports** found none
ready-made, so it is also a build:

- **Boa** (`boa_engine`, pure Rust) targets `wasm32-unknown-unknown` for its browser build; the
  build must take no `js` feature, satisfy `getrandom` with a custom backend returning a fixed
  seed (hash seeds, so no randomness enters), and the result's import section must be empty. Pure
  Rust means the same toolchain as `wasmi` and core, and no C build. Larger and slower than
  QuickJS.
- **QuickJS** (or `quickjs-ng`) needs a libc, and every published wasm build imports WASI. A
  no-import build links stubs for the handful of WASI calls libc reaches (`fd_write` to nowhere,
  `clock_time_get` and `random_get` failing, `proc_exit` trapping) so the import section is
  empty. Smaller and faster; a C build in the pipeline.

Decided and built: **Boa 0.22** (`fixtures/modules/js-engine/`, 3.2 MB), because the build is the
same kind as everything else in the repository and the engine is one
`cargo build --target wasm32-unknown-unknown` away from a checkout. Two things the build taught:
`getrandom` 0.4 refuses the target without a backend, so the custom one answers a fixed sequence;
and Boa's own clock reads the system clock, which the target hasn't (a panic, found through the
module's `last_panic` export), so the realm gets Boa's `FixedClock` at the epoch. The import
section is empty, as core's validation reads. QuickJS stays the alternative when a sample shows
Boa's size or speed is the problem. Either way the rule is one test:
**the engine module's import section is empty**, checked by core's own validation
(`wasm-module-L4.md` §3.1) when the module is added to the store, so an engine that quietly gained
an import cannot be shipped. A WASI build run under stubs the *host* provides is refused: that is
an import section with entries, and the whole argument of `kind_of(module) = Pure` is that core
reads the emptiness from the bytes.

## 5. Values and errors across the boundary

Inputs arrive as core values and are made into JavaScript values; the return travels back. The
mapping, in both directions:

| Core | JavaScript | Notes |
|---|---|---|
| `Number` | `number` | `f64` both sides. `NaN`, `±inf` and `-0` cross as the value encoding canonicalizes them (`value_hash.rs`), so a body that divides by zero answers `Infinity` and the flow sees what `divide` would give. (`wasm-module-L4.md` §3.4 predates the canonical `NaN` in the encoding; the encoding wins.) |
| `Boolean` | `boolean` | |
| `Text` | `string` | UTF-8 both sides; lone surrogates in a JavaScript string are an error, not a mangled text |
| `List` | `Array` | element-wise |
| `Record` | plain object | keys in `BTreeMap` order on the way in; insertion order is lost on the way out, as it is everywhere a `Record` is made |
| absent (an `Option` `none`) | `undefined` | an absent **input** never reaches the body: `list(a, b)` fails closed on an absent input, so the node is absent, as any strict leaf is (an author who wants a default writes `a ?? 0` at the call). A `none` **inside** a list or record arrives as `undefined`. `undefined` or `null` **returned**, or a missing key among several outputs, is an absent output |
| `Sum` other than `Option` | — | not constructible in the language today; an error if met |
| `Effect` | — | **never crosses, either way** (I7). The engine refuses `TAG_EFFECT` on input and cannot produce one: a `js` node is interior, it cannot source or sink |
| — | `bigint`, `function`, `symbol`, `Date`, `Map`, `Set`, class instances, `Promise` | an error naming the kind: "a js node returned a Map; return an array or an object" |

**Errors are values, then absence.** The engine's one output is a record, `{ value }` on success
and `{ error: "<name>: <message> at line L" }` on a thrown exception, a compile error, or a
value that cannot cross. The composite takes `.value`, so from the flow's side an error is an
absent output, exactly like a failed guard or a `json_parse` of bad text, and `missing:` on a
`respond` answers it. The run records the engine's whole output, so `explain` prints the error
beside the statement ("`score` gave nothing: TypeError: estimates.reduce is not a function at
line 2"), which is what the author needs and what a bare module cannot give. Fuel or memory
exhaustion is reported by the engine hook, not the body, and prints as "`score` gave nothing:
out of fuel (50M)".

## 6. Totality, budgets, the manifest (I8)

- Fuel and memory are the engine instance's frozen params (§3), metered by `wasmi` as
  `wasm-module-L4.md` §3.5 says. A `while (true)` in a body runs out of fuel and is absent. A body
  that allocates past the cap is absent. Both are recorded with the reason.
- The manifest gains nothing but what the module unit already contributes:
  `Limit(module_fuel=N)` and `Limit(module_memory=N)`, the **largest** budget any node in the
  flow declares. The approval dialog words them as one line: *runs code it carries, up to N
  instructions and M MiB per call, reaching nothing outside*. The last clause is derived (the
  import section), not promised.
- A flow whose only new thing is a `js` node under the default budgets therefore asks for
  **nothing a flow of leaves would not**, beyond the two limits; and a host policy may cap them
  (`[limits] module_fuel = …`) as it caps request timeouts, refusing a deploy over the ceiling
  with the sentence `deploy` already uses for refusals.

## 7. `check`, `explain`, the editor

- **`check`** validates the body at compile time: the engine is run once with the source and no
  call, so a syntax error is reported with the body's line and column mapped into the file
  ("text:14:9: js `score`: SyntaxError: Unexpected token"). The `.holon` text's spans cover the
  body, so the language server underlines the right place.
- **`explain`** prints the `js` statement as written, its ports with the values the run recorded,
  and, when the output is absent, the engine's error (§5). It does not step inside the body. That
  is the cost named in `wasm-module-L4.md` §1 and accepted: a node is a black box to replay, and
  the remedy is small nodes, which the samples model.
- **The editor** shows a `js` node as a holon-shaped node with its ports, an ⓘ saying it is
  code (engine, size, budgets, hash), the source read-only in view mode and editable in the text
  pane. Design mode runs it for real. *Deviation, built:* the engine's bytes ride in
  `holon-author` (through `holon-module`), so they are in the bridge's bundle (6 MB, from 1.9),
  rather than fetched by hash: lowering must store the engine unit wherever a text is compiled,
  the text pane included, and a unit that isn't in the store can't be placed. Fetching by hash
  would need the lowering to pin a hash it can't verify; revisit if the bundle hurts.
- **`holon show`** prints a `js` node as its statement header and the body's line count.

## 8. The other languages, in a sentence each

- **A compiled module, any language:** `module chess = "chess.wasm" (fen: Text, move: Text) ->
  (fen: Text, status: Text, san: Text)`, from `wasm-module-L4.md` §6; the bytes are stored under
  their hash and the text names the hash once added. Rust, C#, Zig and Go reach the same place
  through their own toolchains.
- **`rust`** is `module` plus a build recipe: `holon build leaves/chess` compiles a crate with a
  pinned toolchain to `wasm32-unknown-unknown`, checks the import section, stores the module and
  writes the `module` line; a `holon-leaf` crate with a `#[leaf]` macro hides the ABI, and
  `Option` on the Rust side is absence. Deferred until a sample exhausts fuel in `js`, which is
  the honest trigger for speed.
- **Lua, Python, PHP, Ruby:** each is another engine module under §4's contract and §5's
  mapping, with its own prelude list, and a statement of the same shape. Nothing in core, the
  manifest, the store or replay knows which language a module carries.

## 9. The sample: `nearest`

Per `CLAUDE.md`, the feature lands with a flow that uses it, in `crates/holon-author/samples/`,
compiled from text against the live catalog, built by `samples.test.ts`, deployed by the
end-to-end tests, and run live before the commit. `nearest` keeps a collection of words and
answers the closest one to a query by edit distance, the two-dimensional table the vocabulary has
no sane spelling for:

```
flow nearest
port 8109
collection words

js closest = (word, candidates) -> (match, distance) {
  const dist = (a, b) => {
    const d = Array.from({ length: a.length + 1 }, (_, i) => [i]);
    for (let j = 1; j <= b.length; j++) d[0][j] = j;
    for (let i = 1; i <= a.length; i++)
      for (let j = 1; j <= b.length; j++)
        d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    return d[a.length][b.length];
  };
  let best = { match: undefined, distance: Infinity };
  for (const c of candidates) { const k = dist(word, c); if (k < best.distance) best = { match: c, distance: k }; }
  return best;
}

route POST /words (req) {
  words[req.body] = { added_at: now }
  respond { added: req.body }, status: 201
}

route GET /nearest/:word (req) {
  found = closest(req.params.word, keys(words.list(limit: 100)))
  respond { query: req.params.word, match: found.match, distance: found.distance }, missing: 404
}
```

The whole flow reads as a flow; the one dense computation is one node; the manifest is the
collection, the clock, requests, and the two default limits. An empty dictionary leaves `match`
`undefined`, so the route answers 404 through `missing:`, with no code for that case anywhere.

## 10. Build order

*Done 2026-10-25:* inches 1 and 2 as written, and of inch 3 the approval wording (`check`, the
agent's `deploy` and the server's manifest say the budgets), `holon show`, the sample `nearest`
(registered, built by `samples.test.ts`, deployed and called live: `kittne` → `kitten` at 2,
`smitten` → `mitten` at 1, `explain` showing the js statement with its values). *Owed:* `check`
compiling the body once for a syntax error at its line (today a body's mistakes surface at run,
as the engine's error, which `explain` can't yet show because the host doesn't record
`WithEngine::take_failure` beside the run: that record is the next inch); the editor's ⓘ for a
module node; the sentence for design doc §13; §14 of `docs/LANGUAGE.md`.


After `wasm-module-L4.md` inches 1 to 3 (the module body and tag, validation, `Pure`, the
`run_module` hook; `holon-module` on `wasmi` with the ABI and budgets; host and bridge wiring and
the limits in the manifest), each inch with its tests, a live check and a commit:

1. **The engine module.** The Boa build to `wasm32-unknown-unknown` with the prelude of §4, the
   ABI of `wasm-module-L4.md` §3 over `encode_value` / `decode_value`, the error record of §5;
   the import-section test; one conformance test per row of §5's table and per prelude removal;
   the `.wasm` committed under `fixtures/modules/js-engine/` with its build script, so a checkout
   needs no toolchain. The catalog's engine entry (hash, default budgets) for `holon-author`.
   Two to three days.
2. **The statement.** Lexer (the balanced body), parser, lowering of §3 including budget
   instantiation, the printer's recognition, `fmt` stability over the samples, the diagnostics
   of §7 with spans into the body, `check` compiling the body once. Golden tests: a `js` node
   lowers to the same hash from the natural form and from the wiring form. Two days.
3. **Surface and sample.** The approval wording, `explain`'s error line, the editor node, the
   engine fetched by hash in design mode, `holon show`; the `nearest` sample, its entry in
   `samples.rs`, `samples.test.ts`, the end-to-end deploy, run live; the sentence for design doc
   §13 that `wasm-module-L4.md` §6 asks for, and §14 of `docs/LANGUAGE.md` pointed here. Two
   days.

## 11. Refused, with the reason

| Refused | Why |
|---|---|
| A native JavaScript engine in the host | It would be faster to build and run, and "computes only" would become a promise instead of a property read from bytes; determinism per platform; a second primitive the day wasm lands |
| Any import, "just" a clock or a logger | `wasm-module-L4.md` §3.1. The one rule the manifest's truth rests on; a flow that wants the time wires `now` |
| `fetch`, storage, timers, `async` in a body | There is nothing to reach and no event loop to wait on; effects are the flow's, declared by leaves, performed by the host |
| State across calls, a shared realm | Two equal calls must be equal; a cached realm is the first place a nondeterminism would hide |
| A `js` node as a route or a flow | A node has ports; routes, collections, state, secrets and awaits are the flow's, and that split is the product |
| The engine hash in the text (`js@…`) | Old files would stop lowering after each engine release; the document's closure pins it exactly already, and `module` by hash exists for whoever must pin |
| A `script` primitive with a host API | The sentence `wasm-module-L4.md` §6 asks to add to design §13, and the end of the thesis if it were built |

## 12. Open forks

- **F1. Engine.** *Decided (2026-10-25):* Boa 0.22 (§4). QuickJS with linked WASI stubs if size
  or speed demands; the numbers to beat are in §2.
- **F6. Compiling a body to a module at deploy.** An ahead-of-time JavaScript-to-wasm compiler
  (Porffor is the serious one) would make the `js` statement another way of producing a
  `module` unit, with the compiled bytes as identity and a pinned compiler's hash in the
  document: no double interpretation, nothing to change in core. Not until a compiler covers
  the language a body may use, since a fallback to the interpreter would give one statement two
  semantics. Until then, the engine caches compiled modules per process; caching Boa's own
  bytecode per source hash is the cheaper step if startup cost shows.
- **F2. Several outputs: object or array.** An object keyed by port name (chosen: readable, and
  a missing key is one absent port, not a shifted list). Revisit only if a body ever needs
  positional outputs.
- **F3. Body size.** No limit beyond the preimage's field budget (`FieldTooLong`, 32-bit). A
  flow with a thousand-line body is a flow that should be a `module`; the editor may warn, the
  language does not refuse.
- **F4. Captures in a body.** A body sees its parameters only; a name from the flow around it
  is not visible and never will be, since the source is one text. Unlike lambdas
  (`docs/LANGUAGE.md` §5.9), nothing is lifted: an author passes what the body needs. The error
  for an unbound name is the engine's `ReferenceError`, printed by `explain`.
- **F5. One engine per language or one engine for all.** One module per language, each under
  §4's contract. A polyglot engine would couple their releases for no gain.

## 13. Invariant check

- **I1 interface parity:** a `js` node is a composite over a module unit; `eval` is the one entry
  point; the printer reads it back. ✓
- **I2 purity:** no imports; a fresh realm per call; no clock, randomness or locale in the realm;
  values canonical at the boundary. ✓
- **I3 by reference:** the engine is a child by hash; the source is a `const` param, as every
  literal is. ✓
- **I5 content addressing:** source, ports, budgets and engine are all in the preimage. ✓
- **I6 transitive pinning:** the document's closure carries the engine; an engine release is a
  new unit, never a changed one. ✓
- **I7 the effect boundary:** `Effect` never crosses; a `js` node is interior. ✓
- **I8 derived labels:** `Pure` from the empty import section, read by core; the two limits from
  the instance's params. ✓
- **One evaluator:** core evaluates units; one engine module, the same bytes everywhere,
  evaluates bodies; the editor fetches those bytes rather than carrying a second engine. ✓
- **Totality:** fuel and memory in the identity; an exception is absence with a reason. ✓
