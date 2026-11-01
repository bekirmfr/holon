# Declared names: the header names everything that outlives a run (L4)

**Status:** designed, not built (2026-11-01). Claims about the code are checked at `646b385` and
cited by file and line there.

A flow's text names the outside world in two ways today. A collection, folder, database, channel
or called flow is declared in the header (`collection notes`) and used by its bare name
(`notes[k]`). A secret or a state key is never declared: `secret.signing_key` and `state.beats`
name themselves where they are used. This note makes it one rule:

> **The header names everything that outlives a run or comes from outside it. The body uses only
> names the header gave.**

So `secret signing_key` and `state beats` become header lines, as `collection notes` is. Uses keep
their namespace (`secret.signing_key`, `state.beats`): the prefix tells a reader, at the line, that
the value is a secret or kept state.

```
flow sign-payload
port 8107
secret signing_key

route POST /sign (req) {
  respond { signature: hmac_sha256(secret.signing_key, req.body), body_base64: base64(req.body) }, missing: 400
}
```

---

## 1. What is there now

**The spec says headers are names for the outside world.** `docs/LANGUAGE.md` §5.1: "Headers are
the flow's deploy settings, exactly as today (`Settings`), and its names for the outside world."

**What the language actually does is a parsing rule.** A store the author names is declared,
because its uses are bare names a parser must tell from a local (`notes[k]`, `shop.query(…)`,
`updates.send(…)`, `billing.charge(…)`). The built-in namespaces (`secret`, `state`, `req`, `now`,
`random`, `ingress`, `http`, `stripe`) are never declared, nor the keys inside them.

- Headers parse at `crates/holon-author/src/lang/parse.rs:280` (the words) and `:337-343` (each
  kind); `HeaderKind::Collection | Folder | Database | Channel | OnFile` lower to nothing of their
  own (`lower.rs:490`): they only make the name usable.
- An undeclared store is an error with its fix: `"x" is not a declared collection (add
  `collection x` to the header)` (`lower.rs:797`).
- `secret.x` lowers to `secret{name: x}` (`lower.rs:1463`), `state.k` to `store_read{key: k}`
  (`lower.rs:1464`), `state.k = e` to `store_write{key: k}` (`lower.rs:701`, `:1855-1864`); no
  check against any declaration.
- The printer derives every declaration from the nodes: `collect_collections` walks the leaves
  (holons' included) for the param that names a collection, folder, database, channel or flow
  (`print.rs:491-530`), and the header lines are written from what it found (`print.rs:318-334`).
  A declaration nothing uses compiles and `holon fmt` drops it (checked: `collection notes` in a
  flow that never uses it).
- **A secret already has a header, but only to say it isn't needed.** `optional secret.x` parses
  through `consent()` (`parse.rs:402-418`) and becomes the deploy setting `optional
  SecretRead("x")` (`lower.rs:485-489`); the printer writes it back from `requirements.optional`
  (`print.rs:305-307`). A flow with such a secret runs without it, and an agent's deploy can
  withhold it (`deploy.rs:530`).
- The deploy finds the secrets a flow reads from its manifest (`SecretRead("…")`) and lists the
  ones not set as `missing_secrets` (`deploy.rs:535-540`): a misspelt `secret.signing_kye` is a new
  required secret, seen only at deploy, as "Not set yet: signing_kye".
- A misspelt state key (`state.beat` beside `state.beats`) is never seen: it is a new key, empty.

**Uses today, in the samples** (`crates/holon-author/samples/`, 46 files): 8 read secrets (14
uses), 9 use state (10 distinct keys).

## 2. Why the parsing rule is the wrong one

The reasons a declaration is worth a line are all stronger for secrets and state than for the
collections that have one:

1. **A secret is a requirement.** Someone other than the author sets it before the flow works.
   That is the thing a reader most needs at the top, and the one the header doesn't show.
2. **Typos.** A declared collection catches `notse[k]` at compile time. `secret.signing_kye` is
   caught late (deploy), `state.beat` never.
3. **The header as the flow's contract.** With every outliving name declared, the top of a file is
   the complete list of what the flow keeps and what it needs provided: the review an approver or
   an auditor does, without reading the body. This is the governance pitch (payment ops reading
   what a flow touches before approving it).
4. **The `optional` line reads backwards today:** a secret appears at the top only to say it can
   be done without.

## 3. The rule, and where each name falls

**Declared in the header (one per line, as collections are):**

| Header | Used as | New? |
|---|---|---|
| `secret signing_key` | `secret.signing_key` | new |
| `optional secret receipt_key` | `secret.receipt_key` | declares it too (today only optional) |
| `state beats` | `state.beats`, `state.beats = e`, `state.set(beats, e)` | new |
| `collection notes` | `notes[k]`, `notes.put(…)`, … | as now |
| `folder inbox`, `on file inbox …` | `inbox[n]`, … | as now |
| `database shop` | `shop.query(…)`, … | as now |
| `channel updates` | `updates.send(…)`, … | as now |
| `calls billing` | `billing.charge(…)` | as now |

**Not declared, because they belong to one run:** `req` (a route names its own, `(req)`), `now`,
`random`, `ingress`. Keys inside a store (`notes[k]`, a file's name) are values, not names, and
stay undeclared, as today.

**HTTP hosts stay inline.** `http.post("hooks.slack.com", …)` names the host as an argument, the
approval shows it (`HttpRequest("hooks.slack.com")`), and `optional HttpRequest("…")` already
exists for one the flow can do without. A host is a value the call is made with, not a name the
flow introduces; declaring hosts would add a line per host for no check the approval doesn't
already make. *This is the one judgement call in the note: a reviewer may decide otherwise.*

## 4. The language change

**Parse** (`parse.rs`):

- `secret <ident>` and `state <ident>` become headers, recognised as the others are: the word,
  then not a `.` (the test `parse.rs:280` makes for every header word, so `state.x = e` at the
  top level stays a statement).
- `optional secret <ident>` is the new spelling; `optional secret.<ident>` keeps parsing, as the
  same thing (fmt rewrites it). Other consents (`optional HttpRequest("…")`) are unchanged.
- New `HeaderKind::Secret(name)`, `HeaderKind::State(name)`; `Consent::Secret` stays for
  `optional`.

**Lower** (`lower.rs`):

- Collect the declared secrets (from `secret` and `optional secret` lines) and state keys, as the
  stores are collected.
- `ExprKind::Secret(n)` not declared: `"signing_key" is not a declared secret (add `secret
  signing_key` to the header)`, at the use, as `lower.rs:797` does for a collection. The same for
  state, reads and writes alike: `"beats" is not declared state (add `state beats` to the
  header)`.
- Inside a holon (`holon … { }`), the flow's declarations apply, as collections do.
- A declaration nothing uses: as a collection today, it compiles and fmt drops it. (The printer
  derives headers from nodes, §5, so this is what "fmt drops it" means; no new warning.)
- `secret` and `state` stay nouns and namespaces (`lower.rs:79`); the header word is recognised
  only at the start of a line, before an identifier.

**Print** (`print.rs`): no new document field. `collect_collections` also gathers the `name` of
each `secret` leaf and the `key` of each `store_read`/`store_write` (holons' leaves included), and
the header writes `secret …` and `state …` lines from them. A secret that is also optional is
written once, as `optional secret x`. Order in the header: `flow`, `port`, `every`, `keep`,
`browsers`, then `secret` and `optional secret` (by name), `state` (by name), then the stores as
today (`on file`, `collection`, `folder`, `database`, `channel`, `calls`), so a file's contract
reads requirements first, then what it keeps.

**The canvas and the web editor:** nothing changes in the document. A `secret` node already
carries its name and a state node its key; the text is derived from them.

## 5. Migration

- **A saved text without the new headers stops compiling.** Where a flow's text is shown, a saved
  text that doesn't compile already gives way to the text printed from its canvas
  (`agent.rs:837-849`, `text_of`), and that print now carries the headers: the editor, `holon
  flows show` and the agent tools show a correct text without anyone migrating it. Deployed flows
  keep running: the server runs their documents, not their text.
- **`holon fmt` adds them.** fmt prints from the compiled document, which needs the text to
  compile, so fmt gets one fix-it: an undeclared secret or state key in an otherwise valid file
  is declared for you, and fmt says what it added (`holon fmt` reports "declared secret
  signing_key, state beats"). The language server offers the same fix as a code action on the
  error.
- **Files on disk** (samples, a user's `.holon` files): `holon fmt` once.

## 6. What changes, and where

- `crates/holon-author/src/lang/{parse,lower,print,ast}.rs`: §4.
- `crates/holon-author/src/editing.rs` (completions, hover): `secret` and `state` as header words;
  after `secret.` and `state.`, offer the declared names first.
- The language server (`docs/L4s/language-server-L4.md`): the fix-it as a code action.
- `crates/holon-author/samples/*.holon`: every sample that reads a secret or uses state, rewritten
  with `holon fmt` (CLAUDE.md: kept as fmt writes them; `editor/src/samples.test.ts` builds each).
  The samples' comment blocks that list secrets to set can then point at the header.
- `crates/holon-author/src/agent.rs`: the reference text the agent tools serve lists the nouns
  (around `agent.rs:580`); it says secrets and state keys are declared.
- `docs/LANGUAGE.md`: §5.1 (the grammar's `header` gains `"secret" ident | "state" ident`, and the
  rule of §3 stated in words), §5.4 (the noun table notes the declaration), the examples
  (guarded-api, slugs), and the keyword list in §4.
- `docs/manual/`: the language pages; `docs/manual/language/leaves.md` regenerates
  (`HOLON_BLESS=1`) if any leaf's wording changes.
- `crates/holon-wasm`: the editor's bridge compiles text, so its tests run (`wasm-pack test
  --node crates/holon-wasm`), and the editor's `editor/wasm` is rebuilt before vitest.

## 7. Slices

1. **Secrets.** Header, `optional secret x`, the error with its fix, print, fmt's fix-it, samples
   that read secrets, LANGUAGE.md for secrets. A sample flow shows it: sign-payload already does
   once rewritten (CLAUDE.md: a change to a feature updates the samples that use it).
2. **State.** The same for `state`, and the samples that use it.
3. **The language server's code action** and the editor's completions.

Each slice is one commit, run live (deploy a rewritten sample, send it a request) before it.

## 8. Tests

- Parse and lower: a declared secret and state key compile; an undeclared one is an error at the
  use with the fix in its message; `optional secret.x` and `optional secret x` lower to the same
  settings; a holon uses the flow's declarations.
- Print: a document with `secret` and `store_read`/`store_write` nodes prints their headers in the
  order of §4; round trip text → document → text is identity for every sample (the existing
  fmt-stable test).
- fmt: a file missing a declaration gets it, and says so; a file with an unused one loses it.
- `text_of`: a saved text from before (no headers) shows as the printed text with them.
- The editor's samples test and the e2e deploys (CI), unchanged in shape.

## 9. Open questions

- **HTTP hosts** (§3): inline, as argued; decide before slice 1 lands.
- **Several names on one line** (`secret api_token, receipt_key`): collections are one per line
  (§5.1); keep it so unless the samples show a long list is common.
- **State keys written by a holon used in several flows:** the holon's flow declares them, as with
  collections today; no holon-level declaration.
