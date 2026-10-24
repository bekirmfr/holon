# `match`: choosing one value by another, without one bad arm spoiling the rest (L4)

**Status:** design, not built (2026-10-25). A language construct only: it lowers to leaves that
exist (`equals`, `guard`, `or`, `not`, `coalesce`), so the hash and the host don't change. It
relies on lazy choices (lazy-choices-L4.md), which land first, so that an arm not chosen doesn't
run (§7).

---

## 1. The problem

A flow that does one of several things by a value (an interpreter's operator, a webhook's event
type, a command's name) has two spellings today, and both fail on absence:

- **A record as a table:** `ops = { add: a + b, div: a / b }` then `ops[op]`. Every entry is
  computed, and a record literal with one absent field is absent itself, so `b = 0` makes `div`
  absent and `add` stops working too. Authors patch every entry with `?? 0`, which then answers
  zeros nobody asked for (the remark's `over` pushing a zero).
- **A chain of `if`:** `if op == "add" then a + b else if op == "div" then a / b else 0`.
  `if` is a fallback, not a branch (LANGUAGE.md §6.4): when `op` is `"div"` and `a / b` is absent,
  the chain falls through to the `else` and answers `0`, silently. The chosen arm failed, and the
  flow says it succeeded.

What's wanted is a choice where the arms not chosen don't matter, and the chosen arm's failure is
the result's failure.

## 2. Syntax

```
result = match op {
  "add" -> a + b
  "sub", "minus" -> a - b
  "div" -> a / b
  else -> 0
}
```

- `match <subject> { <arms> }` is an expression, parsed as an atom like `if`, so it goes anywhere
  an expression does: `respond match op { … }, missing: 400`, `x = match … ?? 0`.
- An arm is `<patterns> -> <expression>`. Arms are separated by newlines or commas, so a short
  one fits on a line: `match b { true -> "yes", false -> "no" }`.
- A pattern is a literal: a number, a text, `true` or `false`. Several, separated by commas, are
  one arm (`"sub", "minus"`). A name is not a pattern; `match` tests a value against values, and a
  condition is `if`'s job.
- `else -> <expression>` is optional, and last. It is chosen when no pattern matched, and only
  then.
- An arm's body is one expression. More than that is a holon, called from the arm.
- `match` becomes a keyword. No sample, test or doc binds the name.

## 3. Meaning

The subject is compared with each pattern by `equals` (structural, never coerced: `1` does not
match `"1"`). The first arm with a matching pattern is chosen.

| Situation | Result |
|---|---|
| The subject is absent | absent (the `else` too) |
| Arm *k* chosen, its value present | that value |
| Arm *k* chosen, its value absent | **absent**: not the `else`, not another arm |
| Any other arm's value absent | no effect |
| No pattern matched, `else` given | the `else`'s value (absent if it is) |
| No pattern matched, no `else` | absent |

The third row is the difference from `if`, and the reason for the construct: an arm that was
chosen and failed is a failure, which `respond … missing:` turns into an answer and view mode
shows at the node where it happened. The fourth row is the difference from a record table.

## 4. Lowering

For a subject `s` and arms with patterns `p₁ … pₙ` (one pattern each, for now):

```
mᵢ    = equals(s, pᵢ)                       one node per pattern, all fed by the one s
armᵢ  = guard(cond: mᵢ, value: eᵢ)
none  = not(or(m₁, …, mₙ))                  only with an else; not(m₁) when n = 1
else' = guard(cond: none, value: e_else)
match = arm₁ ?? arm₂ ?? … ?? armₙ ?? else'  the coalesce chain `??` already lowers to
```

An arm with several patterns has `mᵢ = or(equals(s, p), equals(s, q), …)`. The subject is
lowered once and wired to every `equals`.

Why it gives §3's table:

- `guard` with a false condition is absent, and so is `guard` with an absent value. So every arm
  not chosen is absent whatever its value, and the chosen one is exactly its value.
- The `else` is guarded by "no pattern matched", not left as a plain fallback. When arm *k* is
  chosen, `none` is false and `else'` is absent, so a failed arm can't fall through to it.
- With `s` absent, no `equals` fires, so no `mᵢ`, no `none` and no arm: all absent.
- `or` fires only with every input present; every `mᵢ` is present exactly when `s` is.

Arms are tried in order: if two arms could both match a value, the first wins, because the
coalesce chain takes the first present arm. Two literal patterns that are equal are refused
(§6), so with literal patterns at most one arm matches anyway.

**It is sugar.** A `match` is the same unit as those guards written out by hand: the same nodes,
the same hash. The canvas shows the nodes (`equals`, `guard`, `or`, `not`, `coalesce`); `fmt`
prints them back as `match` (§5).

**Cost.** n arms with an `else`: n `equals`, n + 1 `guard`, one `or`, one `not`, n `coalesce`.
Evaluated: the `equals` of the arms up to the chosen one, and the chosen arm's value. The chain
stops at the first arm present, so later arms (and the `else`) aren't evaluated at all (§7).

## 5. Printing back

The printer recognizes the exact shape of §4: a `coalesce` chain whose links are `guard`s on
`equals` of one shared subject against constants, optionally ending in a `guard` on
`not(or(…))` of those same `equals`. It prints that as `match`. Anything that differs (a guard on
something else, a missing link, a subject that isn't shared) prints as the expressions it is,
`(a + b when op == "add") ?? …`, so the text never claims a `match` the graph isn't. The samples
test holds the sample fmt-stable, as for every sample.

## 6. Errors

| Error | Said at |
|---|---|
| a pattern is a literal (a number, a text, true or false); to test a condition, use `if` | the pattern |
| `"add"` is matched twice; the second arm could never be chosen | the second |
| `else` comes last, once | the second `else`, or the arm after it |
| a `match` needs an arm | the braces |

## 7. Arms that ask the host

With lazy choices (lazy-choices-L4.md), a `guard` evaluates its value only when its condition is
`true`, and a `coalesce` evaluates its second input only when its first is absent. In §4's
lowering, an arm not chosen sits under a `guard` whose condition is false, so it isn't run at
all: its outbound requests aren't sent, its `await`s don't park the run, its reads aren't made,
and a holon it calls isn't entered. An arm may therefore do anything an expression may:

```
sent = match event.type {
  "charge.failed" -> http.post("hooks.slack.com", secret.slack_webhook_path, body: json({ text: "failed: {event.id}" }))
  "charge.refunded" -> http.post("hooks.slack.com", secret.slack_webhook_path, body: json({ text: "refunded: {event.id}" }))
}
```

posts once, for the one event type that came, and not at all for any other.

While the subject waits on an answer (it comes from a request), the first arm's `equals` is
pending, so its `guard` is, and so is the `coalesce` chain: it waits, and no arm runs and no
`else` runs early. Once the subject is known, the chosen arm runs, in the next round.

The first draft of this design refused requests and `await`s in arms, because every arm was
computed, and proposed the same refusal for `if`, which had the same leak. Lazy choices fix the
leak for `if`, `when`, `??` and `match` alike, so neither refusal is needed.

## 8. Alternatives

- **A `switch` leaf whose arms are functions.** Core would evaluate only the chosen arm, as `map`
  applies its function, with the arms riding params as unit hashes. Lazy choices give the same
  laziness to the guards `match` already lowers to, and to `if` and `??` as well, without a leaf
  that holds several functions. Not needed.
- **Make `if` strict.** It would change the meaning of every `if` already written, and the strict
  three-way choice exists already as `select`. `if` keeps its one meaning (§6.4).
- **Lazy fields in record literals.** It would make a record's value depend on which field is read
  later, which a dataflow of values doesn't have.
- **Patterns beyond literals** (ranges, record shapes, bindings). Not needed by any flow yet. The
  syntax leaves room: a pattern position that isn't a literal is an error today, not something
  else.

## 9. What ships with it

- **Parser:** the `match` atom, its arms, the `match` keyword.
- **Lowering:** §4 and the errors of §6.
- **First:** lazy choices (lazy-choices-L4.md), built and released before `match`.
- **Printer:** §5.
- **Tests:**
  - lowering: a `match` is the same unit as the guards written by hand;
  - evaluation: every row of §3's table, through the lowered unit;
  - errors with their columns;
  - fmt round trips.
- **Sample flow:** `calc`.
  ```
  route POST /calc (req) {
    a = req.json.a
    b = req.json.b
    respond match req.json.op {
      "add" -> a + b
      "sub" -> a - b
      "mul" -> a * b
      "div" -> a / b
    }, missing: 400
  }
  ```
  It shows both halves: `{"op":"add","a":1,"b":0}` answers 1 though `a / b` is absent, and
  `{"op":"div","a":1,"b":0}` answers 400, not some other arm's value. Run live, and deployed by
  the e2e tests with a request for each case, where view mode shows the arms not chosen as not
  run.
- **Docs:** LANGUAGE.md gets a §5.6 row and a section of its own beside §6.4 (`if` is a
  fallback, `match` is a choice), and its keyword list gains `match`.
