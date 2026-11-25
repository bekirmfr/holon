//! Where a flow's step runs on a path its text says the run never takes, or doesn't run where it
//! says it does: the debts of "reached", one smallest flow each.
//!
//! A flow reads as JavaScript and is run as a graph, where a step not reached is one whose
//! trigger was tied to what skips it. Where the tie is missing, or tied to the wrong thing, the
//! flow does what its text doesn't say. Each such difference is a named debt here, with the
//! smallest flow that shows it, the one request that shows it, what JavaScript does with that
//! request (written as marks: what the run started, sent, wrote, read and answered, and whether
//! it parked) and what the flow does today. The table holds itself:
//! - a row that no longer differs fails until its debt is struck (each step of the reached rule
//!   strikes its rows in its own commit);
//! - a row that differs in another way than recorded fails;
//! - a row whose text the round trip refuses (`lang::compile`) fails: since C3, when a list's
//!   function hands out what it makes, the round trip takes every row.
//!
//! Seven of the debts have a long form: the same flow written so that today's compiler gets it
//! right. It does what JavaScript does with it today, and is the target of its debt's step: once
//! the rule is in, the short form must behave as its long form does.
//!
//! The marks are read from the run's record: `started` the runs of the flow's actions, `sent` a
//! channel's sends and `wrote` the state written (in any of the request's runs), `read` the reads
//! a run asked of a collection, `parks` a run waiting.

use super::*;

/// A named difference between a flow and what its text says, and the one sentence that says it.
pub(super) struct Debt {
    pub(super) name: &'static str,
    pub(super) says: &'static str,
}

// S (a failed try keeps what its functions made) and the started half were struck by C7
// (2026-11-25): a try works as JavaScript's does and keeps what its block did above the line that
// failed, a function's writes and starts with the rest, so neither is a difference any more.
// L (what a list's function makes is dropped) was struck by C3: a list's function hands out
// what it makes, once per item (core's `did`).
/// G, new in the list grid (2026-11-22): a list's function goes on past an item that fails.
pub(super) const A_LISTS_FUNCTION_GOES_PAST_A_FAILURE: Debt = Debt {
    name: "a list's function goes on past an item that fails",
    says: "map, filter, groupBy and the others call their function on the items after one whose function fails, and make its starts and requests there, where JavaScript stops at the item that throws",
};
// "a findLast starts and asks in the items' order" was struck by C3: each application asks at
// the order it is made in.
/// New in the list grid (2026-11-22): a reviver or replacer that gives nothing puts null there.
pub(super) const A_REVIVERS_NOTHING_IS_NULL: Debt = Debt {
    name: "a reviver's nothing is null",
    says: "a JSON.parse reviver or JSON.stringify replacer whose function fails on an item puts null there and the whole succeeds, so a catch around it isn't reached and the items after it are called, where JavaScript throws",
};
/// New in the list grid (2026-11-22): a fold stops calling its function once the result so far
/// is nothing.
pub(super) const A_FOLD_STOPS_AT_NOTHING: Debt = Debt {
    name: "a fold stops at a result that is nothing",
    says: "a reduce whose function gives nothing for an item (text and a start's handle added) makes no start for the items after it, where JavaScript goes on (its sum is a text)",
};
// R173 (a list's function starts where the run doesn't go) was struck by C3: the list call takes
// a trigger, tied as any step that does something.
// W (a finally forgets the returns above), Z (a call in an arm read only in a branch not taken
// isn't made) and Z's one call deeper were struck by C4: a finally is reached where its try is,
// and a call in a branch is made where the branch is taken, read or not.
/// New in C10 (2026-11-24): a list holding an item that didn't arrive is nothing as a whole, where
/// JavaScript keeps `undefined` there and JSON writes it `null`. Not a reached difference: what a
/// missing value is belongs to the stages that say it (text-means-javascript S3, S5).
pub(super) const A_LIST_WITH_A_MISSING_ITEM_IS_NOTHING: Debt = Debt {
    name: "a list holding a missing item is nothing",
    says: "a list one of whose items is missing (a field the request lacks, or a list function's answer for an item) has no value, so JSON.stringify of it gives nothing and the route answers 404, where JavaScript writes the missing item as null ([null,2])",
};
// P+R (a wait past a return that answered still waits) and its sibling (a wait past a test with
// no value still waits) were struck by C6: a wait is a step that does something, tied to where its
// line is reached, so a run that answered above it, or ended there, isn't parked.
/// A: an arm's test is read before its statement is reached.
pub(super) const A_CHOICES_TEST_IS_READ_PAST_A_RETURN: Debt = Debt {
    name: "the arm's test is read before the statement's reached",
    says: "a ?: whose test reads a collection asks that read below a return that answered",
};
/// New in the grid (2026-11-22): R's other side, a read left unasked where JavaScript asks it.
pub(super) const AN_UNUSED_READ_IS_NOT_ASKED: Debt = Debt {
    name: "a read nothing reached goes on to use isn't asked",
    says: "a collection read whose value is read only in a branch not taken, or an if's test read where nothing in the if is made, is not asked, where JavaScript asks it",
};
/// R: a read is asked where its statement isn't reached.
pub(super) const A_READ_IS_ASKED_PAST_A_RETURN: Debt = Debt {
    name: "a read is asked where its statement isn't reached",
    says: "a collection read in a send's content is asked below a return that answered, though the send is held; so is a folder's file read for an upload under an if that doesn't hold (C1's site runs, 2026-11-23)",
};
/// B: a line after a failure outside a try still runs. The owner's question, rule or debt: a debt
/// until it is decided.
pub(super) const A_LINE_AFTER_A_FAILURE_RUNS: Debt = Debt {
    name: "a line after a failure outside a try still runs",
    says: "a write on the line after a JSON.parse that fails, with no try around them, is made, where JavaScript throws first",
};

/// Every debt of the table, each to be struck when no row differs under it.
pub(super) const DEBTS: &[&Debt] = &[
    &A_CHOICES_TEST_IS_READ_PAST_A_RETURN,
    &A_READ_IS_ASKED_PAST_A_RETURN,
    &A_LINE_AFTER_A_FAILURE_RUNS,
    &AN_UNUSED_READ_IS_NOT_ASKED,
    &A_LISTS_FUNCTION_GOES_PAST_A_FAILURE,
    &A_REVIVERS_NOTHING_IS_NULL,
    &A_FOLD_STOPS_AT_NOTHING,
    &A_LIST_WITH_A_MISSING_ITEM_IS_NOTHING,
];

/// How a row stands today.
enum Today {
    /// The flow does what JavaScript does.
    AsJavascript,
    /// The flow is taken and does these marks instead, under the debt. (Until C3 a row could also
    /// be refused by the round trip and deployed by the reader alone; none is now, and a row the
    /// round trip refuses fails.)
    Differs(&'static Debt, &'static str),
    /// Waiting for the owner: the flow is left doing these marks, as it did before the question
    /// was asked, until it is decided whether this is a difference at all. A row that starts doing
    /// what JavaScript does, or anything else, fails, to be turned to its answer.
    WaitsForTheOwner(&'static str, &'static str),
}

/// The owner's open question, asked with C7 (2026-11-25): in a `try`, does a request, a start or a
/// send written on its own line, its value not bound, run the catch when it itself fails (a host
/// that refuses the connection), as JavaScript's `await fetch(…)` throws there? Left as before C7:
/// such a line is no line that may fail, and the catch doesn't run.
pub(super) const OWN_LINE_FAILURE_RUNS_THE_CATCH: &str = "does a request, start or send on its own line that fails run the catch";

/// One row: a plain name, the short text (its route `POST /go`), the request, whether the answer
/// is compared, what JavaScript does, how it stands today, and the long form with what JavaScript
/// does with it (and so the flow does today).
struct Row {
    name: &'static str,
    text: &'static str,
    request: &'static str,
    answer: bool,
    javascript: &'static str,
    today: Today,
    long: Option<(&'static str, &'static str)>,
}

const PING: &str = "export const ping = action((x) => ++state.pinged)\n";

fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "a function that starts something, called under an if that doesn't hold",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-01\", { port: 19601 })\n\n@PING@\nfunction ask(x) {\n  if (x) {\n    ping(x)\n  }\n  return 1\n}\n\nroute.post(\"/go\", (req) => {\n  if (req.json.go) {\n    const asked = ask(req.json.a)\n  }\n  return \"end\"\n})\n",
            request: r#"{"go":false,"a":"x"}"#,
            answer: true,
            javascript: "answers \"end\"",
            today: Today::AsJavascript,
            long: Some((
                "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-01-long\", { port: 19621 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  if (req.json.go) {\n    if (req.json.a) {\n      ping(req.json.a)\n    }\n  }\n  return \"end\"\n})\n",
                "answers \"end\"",
            )),
        },
        Row {
            name: "a function that writes, in a try that then fails",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-02\", { port: 19602 })\n\nfunction bump(x) {\n  ++state.n\n  return x + \"!\"\n}\n\nroute.post(\"/go\", (req) => {\n  try {\n    const n = bump(req.json.a)\n    const parsed = JSON.parse(req.json.text)\n  } catch {\n    return \"bad\"\n  }\n  return \"end\"\n})\n",
            request: r#"{"a":"x","text":"{bad"}"#,
            answer: true,
            javascript: "answers \"bad\"; writes n = 1",
            today: Today::AsJavascript,
            long: Some((
                "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-02-long\", { port: 19622 })\n\nroute.post(\"/go\", (req) => {\n  try {\n    ++state.n\n    const parsed = JSON.parse(req.json.text)\n  } catch {\n    return \"bad\"\n  }\n  return \"end\"\n})\n",
                "answers \"bad\"; writes n = 1",
            )),
        },
        Row {
            name: "a function that writes, given to a list's map",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-03\", { port: 19603 })\n\nfunction keep(x) {\n  ++state.n\n  return x + \"!\"\n}\n\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a].map((x) => keep(x))\n  return \"end\"\n})\n",
            request: r#"{"a":"x"}"#,
            answer: true,
            javascript: "answers \"end\"; writes n = 1",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a list's function that starts something, below a return that answered",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-04\", { port: 19604 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"early\"\n  const got = [req.json.a].map((x) => ping(x))\n  return \"end\"\n})\n",
            request: r#"{"stop":true,"a":"x"}"#,
            answer: true,
            javascript: "answers \"early\"",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a function whose start is in a choice, called under an if that doesn't hold",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-05\", { port: 19605 })\n\n@PING@\nfunction maybe(x) {\n  return x ? ping(x) : 0\n}\n\nroute.post(\"/go\", (req) => {\n  if (req.json.go) {\n    const made = maybe(req.json.a)\n  }\n  return \"end\"\n})\n",
            request: r#"{"go":false,"a":"x"}"#,
            answer: true,
            javascript: "answers \"end\"",
            today: Today::AsJavascript,
            long: Some((
                "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-05-long\", { port: 19625 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  if (req.json.go) {\n    const made = req.json.a ? ping(req.json.a) : 0\n  }\n  return \"end\"\n})\n",
                "answers \"end\"",
            )),
        },
        Row {
            name: "a write in a finally, below a return that answered",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-06\", { port: 19606 })\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  try {\n    const parsed = JSON.parse(req.json.text)\n  } catch {\n    return \"bad\"\n  } finally {\n    ++state.fin\n  }\n  return \"end\"\n})\n",
            request: r#"{"stop":true,"text":"{}"}"#,
            answer: true,
            javascript: "answers \"stopped\"",
            today: Today::AsJavascript,
            long: Some((
                "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-06-long\", { port: 19626 })\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  if (!req.json.stop) {\n    try {\n      const parsed = JSON.parse(req.json.text)\n    } catch {\n      return \"bad\"\n    } finally {\n      ++state.fin\n    }\n  }\n  return \"end\"\n})\n",
                "answers \"stopped\"",
            )),
        },
        Row {
            name: "an action that answers above its wait",
            text: "import { action, event, flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"reached-07\", { port: 19607 })\n\nexport const pay = action(async (input) => {\n  if (input.stop) return \"stopped\"\n  const ok = await event(\"approved\", { expires: \"1h\" })\n  updates.send({ status: \"paid\", by: ok.by })\n  return \"done\"\n})\n\nroute.post(\"/go\", (req) => {\n  const started = pay(req.json)\n  return started\n})\n",
            request: r#"{"stop":true}"#,
            // What a start answers is the starter's: `{answer, run}` for a run that answered,
            // Holon's own (§5.4), where JavaScript's is a promise; the waits grid holds it. So only
            // what it does is compared: it starts pay, which answers and doesn't park.
            answer: false,
            javascript: "starts pay",
            today: Today::AsJavascript,
            long: Some((
                "import { action, event, flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"reached-07-long\", { port: 19627 })\n\nexport const pay = action(async (input) => {\n  const ok = await event(\"approved\", { expires: \"1h\" })\n  updates.send({ status: \"paid\", by: ok.by })\n  return \"done\"\n})\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  const started = pay(req.json)\n  return started\n})\n",
                "answers \"stopped\"",
            )),
        },
        Row {
            name: "an action whose test above its wait has no value",
            text: "import { action, event, flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"reached-20\", { port: 19620 })\n\nexport const pay = action(async (input) => {\n  if (input.stop) return \"stopped\"\n  const ok = await event(\"approved\", { expires: \"1h\" })\n  updates.send({ status: \"paid\", by: ok.by })\n  return \"done\"\n})\n\nroute.post(\"/go\", (req) => {\n  const started = pay(req.json)\n  return started\n})\n",
            request: r#"{}"#,
            // Holon's promise, not JavaScript's (§5.4): the test with no value ends the run, where
            // JavaScript reads it as false and waits. What a start answers for a run that answered
            // nothing is the starter's, so only what it does is compared.
            answer: false,
            javascript: "starts pay",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a choice whose test reads a collection, below a return that answered",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"reached-08\", { port: 19608 })\n\nfunction rec(x) {\n  ++state.f\n  return x + \"!\"\n}\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  const got = notes[req.json.k] === \"v\" ? rec(req.json.k) : 0\n  return \"end\"\n})\n",
            request: r#"{"stop":true,"k":"a"}"#,
            answer: true,
            javascript: "answers \"stopped\"",
            today: Today::Differs(&A_CHOICES_TEST_IS_READ_PAST_A_RETURN, "answers \"stopped\"; reads notes"),
            long: None,
        },
        Row {
            name: "a start in a choice that is taken, its value read only in a branch that isn't",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-09\", { port: 19609 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  const got = req.json.c ? ping(req.json) : 0\n  return req.json.d ? got : \"no\"\n})\n",
            request: r#"{"c":true,"d":false}"#,
            answer: true,
            javascript: "answers \"no\"; starts ping; writes pinged = 1",
            today: Today::AsJavascript,
            long: Some((
                "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-09-long\", { port: 19629 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  if (req.json.c) {\n    ping(req.json)\n  }\n  return req.json.d ? \"yes\" : \"no\"\n})\n",
                "answers \"no\"; starts ping; writes pinged = 1",
            )),
        },
        Row {
            name: "a send whose content reads a collection, below a return that answered",
            text: "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nimport updates from \"channel:updates\"\n\nflow(\"reached-10\", { port: 19610 })\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  updates.send({ text: notes[\"k\"] })\n  return \"end\"\n})\n",
            request: r#"{"stop":true}"#,
            answer: true,
            javascript: "answers \"stopped\"",
            today: Today::Differs(&A_READ_IS_ASKED_PAST_A_RETURN, "answers \"stopped\"; reads notes"),
            long: None,
        },
        Row {
            name: "a write on the line after one that fails, with no try",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-11\", { port: 19611 })\n\nroute.post(\"/go\", (req) => {\n  const parsed = JSON.parse(req.json.text)\n  ++state.n\n  return \"end\"\n})\n",
            request: r#"{"text":"{bad"}"#,
            // JavaScript throws here; what the route should answer is the owner's open decision,
            // so only what it does is compared.
            answer: false,
            javascript: "does nothing",
            today: Today::Differs(&A_LINE_AFTER_A_FAILURE_RUNS, "writes n = 1"),
            long: Some((
                "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-11-long\", { port: 19631 })\n\nroute.post(\"/go\", (req) => {\n  try {\n    const parsed = JSON.parse(req.json.text)\n    ++state.n\n  } catch {\n    return \"failed\"\n  }\n  return \"end\"\n})\n",
                "answers \"failed\"",
            )),
        },
        Row {
            name: "a list's function that starts something, the list read only in a branch not taken",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-13\", { port: 19613 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a].map((x) => ping(x))\n  return req.json.c ? got : \"no\"\n})\n",
            request: r#"{"a":"x","c":false}"#,
            answer: true,
            javascript: "answers \"no\"; starts ping; writes pinged = 1",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a function whose start is in a choice, its value read only in a branch not taken",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-14\", { port: 19614 })\n\n@PING@\nfunction maybe(x) {\n  return x ? ping(x) : 0\n}\n\nroute.post(\"/go\", (req) => {\n  const got = maybe(req.json.a)\n  return req.json.c ? got : \"no\"\n})\n",
            request: r#"{"a":"x","c":false}"#,
            answer: true,
            javascript: "answers \"no\"; starts ping; writes pinged = 1",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a collection read bound to a name, read only in a branch not taken",
            text: "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\n\nflow(\"reached-15\", { port: 19615 })\n\nroute.post(\"/go\", (req) => {\n  const got = notes[\"k\"]\n  return req.json.c ? got : \"no\"\n})\n",
            request: r#"{"c":false}"#,
            answer: true,
            javascript: "answers \"no\"; reads notes",
            today: Today::Differs(&AN_UNUSED_READ_IS_NOT_ASKED, "answers \"no\""),
            long: None,
        },
        Row {
            name: "a list's function that starts something and then fails, on the first item",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-16\", { port: 19616 })\n\n@PING@\nfunction go(x) {\n  ping(x)\n  const parsed = JSON.parse(x === \"a\" ? \"{\" : \"1\")\n  return parsed\n}\n\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a, req.json.b].map((x) => go(x))\n  return \"end\"\n})\n",
            request: r#"{"a":"a","b":"b"}"#,
            // JavaScript throws at the first item; what the route answers is the owner's open
            // question (B), so only what it does is compared.
            answer: false,
            javascript: "starts ping; writes pinged = 1",
            today: Today::Differs(&A_LISTS_FUNCTION_GOES_PAST_A_FAILURE, "starts ping, ping; writes pinged = 1, pinged = 2"),
            long: None,
        },
        Row {
            name: "a findLast whose function starts something",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-17\", { port: 19617 })\n\nexport const ping = action((x) => {\n  state.seen = (state.seen ?? \"\") + x\n  return x\n})\n\nfunction go(x) {\n  ping(x)\n  return false\n}\n\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a, req.json.b].findLast((x) => go(x))\n  return \"end\"\n})\n",
            request: r#"{"a":"a","b":"b"}"#,
            answer: true,
            javascript: "answers \"end\"; starts ping, ping; writes seen = \"b\", seen = \"ba\"",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a reviver that fails on an item, in a try",
            text: "import { flow, route } from \"holon\"\n\nflow(\"reached-18\", { port: 19618 })\n\nfunction check(x) {\n  const parsed = JSON.parse(x === \"a\" ? \"{\" : \"1\")\n  return parsed\n}\n\nroute.post(\"/go\", (req) => {\n  try {\n    const got = JSON.parse(req.json.text, (k, x) => Array.isArray(x) || k === \"\" ? x : check(x))\n    return got\n  } catch {\n    return \"bad\"\n  }\n})\n",
            request: r#"{"text":"[\"a\",\"b\"]"}"#,
            answer: true,
            javascript: "answers \"bad\"",
            today: Today::Differs(&A_REVIVERS_NOTHING_IS_NULL, "answers \"[null,1]\""),
            long: None,
        },
        Row {
            name: "a reduce that adds a start's handle to a text",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-19\", { port: 19619 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a, req.json.b].reduce((acc, x) => acc + ping(x), \"\")\n  return \"end\"\n})\n",
            request: r#"{"a":"a","b":"b"}"#,
            answer: true,
            javascript: "answers \"end\"; starts ping, ping; writes pinged = 1, pinged = 2",
            today: Today::Differs(&A_FOLD_STOPS_AT_NOTHING, "answers \"end\"; starts ping; writes pinged = 1"),
            long: None,
        },
        Row {
            name: "a list's function that starts something, an item's answer missing",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-21\", { port: 19632 })\n\n@PING@\nfunction go(x) {\n  ping(x)\n  return x.maybe\n}\n\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a, req.json.b].map((x) => go(x))\n  return \"end\"\n})\n",
            request: r#"{"a":{"n":1},"b":{"maybe":2}}"#,
            answer: true,
            javascript: "answers \"end\"; starts ping, ping; writes pinged = 1, pinged = 2",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a list holding a field that's missing, written as JSON",
            text: "import { flow, route } from \"holon\"\n\nflow(\"reached-22\", { port: 19633 })\n\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a, req.json.b]\n  return JSON.stringify(got)\n})\n",
            request: r#"{"b":2}"#,
            answer: true,
            javascript: "answers \"[null,2]\"",
            today: Today::Differs(&A_LIST_WITH_A_MISSING_ITEM_IS_NOTHING, "answers 404"),
            long: None,
        },
        Row {
            name: "a key written, then written again in an if on a field that's missing",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-12\", { port: 19612 })\n\nroute.post(\"/go\", (req) => {\n  state.k = \"two\"\n  if (req.json.missing) {\n    state.k = \"one\"\n  }\n  return \"end\"\n})\n",
            request: r#"{}"#,
            answer: true,
            javascript: "answers \"end\"; writes k = \"two\"",
            today: Today::AsJavascript,
            long: None,
        },
        Row {
            name: "a request on its own line, in a try, to a host that refuses it",
            text: "import { flow, route } from \"holon\"\nimport api from \"host:@REFUSED@\"\n\nflow(\"reached-23\", { port: 19634 })\n\nroute.post(\"/go\", async (req) => {\n  try {\n    await api.post(\"/hit\", { body: req.json.a })\n    const parsed = JSON.parse(req.json.text)\n  } catch {\n    return \"bad\"\n  }\n  return \"end\"\n})\n",
            request: r#"{"a":"x","text":"{}"}"#,
            answer: true,
            javascript: "answers \"bad\"",
            today: Today::WaitsForTheOwner(OWN_LINE_FAILURE_RUNS_THE_CATCH, "answers \"end\""),
            long: None,
        },
    ]
}

/// A value of the run's record as a mark shows it: a whole number without its point, a text in
/// quotes.
fn shown(v: &serde_json::Value) -> String {
    match (v["type"].as_str(), &v["value"]) {
        (Some("Number"), serde_json::Value::Number(n)) => {
            let f = n.as_f64().unwrap();
            if f.fract() == 0.0 { format!("{}", f as i64) } else { format!("{f}") }
        }
        (Some("Text"), serde_json::Value::String(s)) => format!("{s:?}"),
        _ => v.to_string(),
    }
}

/// What `body` sent to `POST <url>` of `slug` did, as marks read from the run's record (every
/// entry the request made: its own run and the runs of the actions it started). `answer` says
/// whether the answer is a mark.
pub(super) fn marks_of(state: &AppState, slug: &str, url: &str, body: &str, answer: bool) -> String {
    let before = log_entries(state, slug).iter().filter_map(|e| e["seq"].as_u64()).max();
    let (status, said, _) = data_call(state, slug, "POST", url, body);
    let entries: Vec<serde_json::Value> = log_entries(state, slug).into_iter().filter(|e| before.is_none_or(|b| e["seq"].as_u64().is_some_and(|s| s > b))).collect();
    let (mut started, mut sent, mut wrote, mut read) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut parks = false;
    for e in &entries {
        if let Some(action) = e["route"].as_str().and_then(|r| r.strip_prefix("action ")) {
            started.push(action.to_string());
        }
        for p in e["performed"].as_array().into_iter().flatten() {
            let capability = p["capability"].as_str().unwrap_or("");
            if let Some(key) = capability.strip_prefix("StateWrite(\"").and_then(|k| k.strip_suffix("\")")) {
                wrote.push(format!("{key} = {}", shown(&p["payload"])));
            } else if let Some(channel) = capability.strip_prefix("ChannelSend(\"").and_then(|k| k.strip_suffix("\")")) {
                sent.push(channel.to_string());
            } else {
                panic!("an effect the table doesn't know: {capability}");
            }
        }
        for r in e["reads"].as_array().into_iter().flatten() {
            read.push(r["request"]["value"].as_array().and_then(|f| f.iter().find(|p| p[0] == "collection")).map(|p| p[1]["value"].as_str().unwrap_or("").to_string()).unwrap_or_else(|| r["request"].to_string()));
        }
        parks |= e["parked"].as_array().is_some_and(|p| !p.is_empty());
    }
    let mut marks = Vec::new();
    if answer {
        marks.push(if status != 200 {
            format!("answers {status}")
        } else if said.contains("\"waiting\"") {
            "answers a waiting link".to_string()
        } else {
            format!("answers {:?}", said.trim_matches('"'))
        });
    }
    for (word, mut list) in [("starts", started), ("sends", sent), ("writes", wrote), ("reads", read)] {
        list.sort();
        if !list.is_empty() {
            marks.push(format!("{word} {}", list.join(", ")));
        }
    }
    if parks {
        marks.push("parks".to_string());
    }
    if marks.is_empty() { "does nothing".to_string() } else { marks.join("; ") }
}

/// **Guards:** each debt of reached is one named difference, held exactly: a row that no longer
/// differs fails until its debt is struck, a row that differs otherwise fails, and every long form
/// does what JavaScript does with it.
///
/// **Why:** the reached rule is built one kind of step at a time; each step strikes the rows it
/// cures, in its own commit, and nothing else may move.
///
/// **If it fails:** "strike it" means a step cured its case: remove the debt and turn its row to
/// `AsJavascript`. "Differs another way" means a change moved what the flow does here without
/// curing it: read the two lines it prints.
#[test]
fn the_debts_of_reached_each_differ_as_recorded() {
    let state = claims_state("");
    let (host, _hits) = counting_server();
    let refused = refusing_host();
    let mut wrong = Vec::new();
    let mut owing: BTreeSet<&str> = BTreeSet::new();
    for row in rows() {
        let text = row.text.replace("@PING@", PING).replace("@REFUSED@", &refused);
        if let Err(e) = holon_author::lang::compile(&text) {
            wrong.push(format!("{}: the round trip refuses it: {e}", row.name));
            continue;
        }
        let slug = deploy_text(&state, &text, if text.contains(&refused) { &refused } else { &host });
        let did = marks_of(&state, &slug, "/go", row.request, row.answer);
        match row.today {
            Today::AsJavascript => {
                if did != row.javascript {
                    wrong.push(format!("{}: JavaScript {:?}, the flow {did:?}", row.name, row.javascript));
                }
            }
            Today::Differs(debt, recorded) => {
                owing.insert(debt.name);
                if did == row.javascript {
                    wrong.push(format!("\"{}\" no longer differs on {:?}: strike it ({})", debt.name, row.name, debt.says));
                } else if did != recorded {
                    wrong.push(format!("{}: differs another way than \"{}\" records: recorded {recorded:?}, the flow {did:?}, JavaScript {:?}", row.name, debt.name, row.javascript));
                }
            }
            Today::WaitsForTheOwner(question, recorded) => {
                if did != recorded {
                    wrong.push(format!("{}: left as it was while the owner decides \"{question}\", it now does {did:?} (recorded {recorded:?}, JavaScript {:?})", row.name, row.javascript));
                }
            }
        }
        if let Some((long, javascript)) = row.long {
            let long = long.replace("@PING@", PING);
            assert!(holon_author::lang::compile(&long).is_ok(), "{}, the long form: {:?}", row.name, holon_author::lang::compile(&long).err());
            let slug = deploy_text(&state, &long, &host);
            let did = marks_of(&state, &slug, "/go", row.request, true);
            if did != javascript {
                wrong.push(format!("{}, the long form: JavaScript {javascript:?}, the flow {did:?}", row.name));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    for debt in DEBTS {
        assert!(owing.contains(debt.name), "\"{}\" has no row: strike it or give it one", debt.name);
    }
}

/// What `state.k` holds after a route that writes it, then writes it again in an `if` on each
/// condition of the grid, for each of the grid's requests: as JavaScript leaves it, and as a flow
/// reads it (the grid's rule for a condition that isn't there: neither branch). Per condition, per
/// request: (JavaScript's, as a flow reads it).
fn one_key_in_javascript() -> Vec<Vec<(String, String)>> {
    use super::semantics::{neither, AS_A_FLOW_READS, CONDITIONS, REQUESTS};
    let names = "state, notes, check";
    let made: Vec<String> = CONDITIONS
        .iter()
        .map(|(written, read, _)| {
            let read = neither(&format!("state.k = \"two\"\nif ({read}) {{\nstate.k = \"one\"\n}}"));
            format!("[({{ {names} }}) => (req) => {{\nstate.k = \"two\"\nif ({written}) {{\nstate.k = \"one\"\n}}\n}}, ({{ {names} }}) => (req) => {{\nconst J = req.json ?? {{}};\nlet IFC;\n{read}\n}}]")
        })
        .collect();
    let source = format!(
        r#"(requests) => {{
  {AS_A_FLOW_READS}
  const handlers = [
{}
  ];
  const run = (make, req) => {{
    const state = {{}};
    const env = {{ state, notes: new Proxy({{}}, {{ get: () => undefined }}), check: (x) => x === "a" }};
    try {{ make(env)(req) }} catch (e) {{ return "threw" }}
    return state.k === undefined ? "none" : JSON.stringify(state.k);
  }};
  return handlers.map(([written, read]) => requests.map((req) => [run(written, req), run(read, req)]));
}}"#,
        made.join(",\n")
    );
    let requests: Vec<Value> = REQUESTS
        .iter()
        .map(|body| {
            let mut req = BTreeMap::from([("body".to_string(), Value::Text(body.to_string()))]);
            if let Some(json) = holon_core::parse_json(body) {
                req.insert("json".to_string(), json);
            }
            Value::Record(req)
        })
        .collect();
    let limits = holon_core::ModuleLimits { fuel: 1_000_000_000, memory: 64 << 20 };
    let outs = holon_module::shared().run(holon_module::JS_ENGINE, &limits, &[Some(Value::Text(source)), Some(Value::List(vec![Value::List(requests)]))]).unwrap();
    let Some(Value::Record(answer)) = outs.into_iter().next().flatten() else { panic!("the engine answers a record") };
    let Some(Value::List(per_condition)) = answer.get("value") else { panic!("JavaScript failed: {answer:?}") };
    let text = |v: &Value| match v {
        Value::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    };
    let list = |v: &Value| match v {
        Value::List(items) => items.clone(),
        other => panic!("{other:?}"),
    };
    per_condition.iter().map(|per_request| list(per_request).iter().map(|both| (text(&list(both)[0]), text(&list(both)[1]))).collect()).collect()
}

/// **Guards:** one state key written before an `if` and again inside it (`state.k = "two"`, then
/// `state.k = "one"` in the `if`) holds what JavaScript leaves in it, or what the flow's own rule
/// for a condition that isn't there leaves (neither branch, so "two"), for every condition form of
/// the grid and every request of it. No debt is owed here: the reading that the `if` undoes the
/// write above it (K) was run on 0.1.0-rc.37 and didn't hold.
///
/// **Why:** a state write's condition lives in its value; the two writes are one key, joined after
/// the `if`. A join that took "not entered" for "nothing" would lose the "two".
///
/// **If it fails:** the line says the condition, the request, and what JavaScript, the rule and
/// the flow left in `k`.
#[test]
fn one_key_written_before_an_if_and_inside_it_holds_its_value() {
    use super::semantics::{CONDITIONS, REQUESTS};
    let state = claims_state("");
    let mut text = "import { flow, route } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"one-key\", { port: 19640 })\n\nfunction check(x) {\n  return x === \"a\"\n}\n".to_string();
    for (i, (condition, _, _)) in CONDITIONS.iter().enumerate() {
        text.push_str(&format!("\nroute.post(\"/k{i}\", (req) => {{\n  state.k = \"two\"\n  if ({condition}) {{\n    state.k = \"one\"\n  }}\n  return \"end\"\n}})\n"));
    }
    let slug = deploy_plain_text(&state, &text);
    let javascript = one_key_in_javascript();
    let (mut as_javascript, mut by_rule, mut wrong) = (0, 0, Vec::new());
    for (i, ((condition, _, _), said)) in CONDITIONS.iter().zip(&javascript).enumerate() {
        for (request, (written, read)) in REQUESTS.iter().zip(said) {
            let before = log_entries(&state, &slug).iter().filter_map(|e| e["seq"].as_u64()).max();
            data_call(&state, &slug, "POST", &format!("/k{i}"), request);
            let entries = log_entries(&state, &slug);
            let ran = entries.iter().find(|e| before.is_none_or(|b| e["seq"].as_u64().is_some_and(|s| s > b))).expect("the run's entry");
            let wrote: Vec<String> = ran["performed"].as_array().into_iter().flatten().filter(|p| p["capability"] == "StateWrite(\"k\")").map(|p| shown(&p["payload"])).collect();
            let did = wrote.last().cloned().unwrap_or_else(|| "none".to_string());
            if did == *written {
                as_javascript += 1;
            } else if did == *read {
                by_rule += 1;
            } else {
                wrong.push(format!("{condition} | {request} | JavaScript {written}, as a flow reads it {read}, the flow {did}"));
            }
        }
    }
    println!("one key: {as_javascript} as JavaScript, {by_rule} by the flow's own rule");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(as_javascript > by_rule, "{as_javascript} as JavaScript, {by_rule} by rule");
}

/// A local address nothing listens on: a request to it is refused at once.
fn refusing_host() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    drop(listener);
    host
}

/// The kinds of thing a `try` block changes, for the join sweep: a name, the statement that
/// changes it to the request's `one`, what reads it (`{R}`, what a catch or the code after the
/// try answers), and what sets it before the try, if anything.
const JOIN_KINDS: &[(&str, &str, &str, &str)] = &[
    ("a local name", "v = req.json.one", "v", "let v = req.json.zero\n  "),
    ("a state key", "state.k = req.json.one", "\"end\"", ""),
    ("a state key written twice", "state.k = req.json.one", "\"end\"", "@IN@state.k = req.json.zero\n    "),
    ("a collection item", "notes[req.json.key] = req.json.one", "\"end\"", ""),
    ("a delete", "delete gone[req.json.key]", "\"end\"", ""),
];

/// The ways a line of a `try` block fails, each failing on the sweep's second request and not on
/// its first.
const JOIN_FAILURES: &[&str] = &["const p = JSON.parse(req.json.text)", "const p = req.json.a.b", "if (req.json.bad) throw new Error(\"no\")"];

/// The sweep's requests: one on which no line fails, one on which each does.
const JOIN_REQUESTS: &[&str] = &[r#"{"zero":"zero","one":"one","key":"k","text":"{}","a":{"b":1},"bad":false}"#, r#"{"zero":"zero","one":"one","key":"k","text":"{bad","bad":true}"#];

/// The sweep's handlers: each kind, changed before or after the failing line (a key written twice:
/// before and after), read by the catch or by the code after the try, for each way to fail.
fn join_handlers() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (kind, change, read, before) in JOIN_KINDS {
        let (outside, inside) = match before.strip_prefix("@IN@") {
            Some(inside) => ("", inside),
            None => (*before, ""),
        };
        let places: &[&str] = if inside.is_empty() { &["before", "after"] } else { &["around"] };
        for place in places {
            for reader in ["the catch", "after the try"] {
                for failure in JOIN_FAILURES {
                    let block = match *place {
                        "before" => format!("{change}\n    {failure}"),
                        "after" => format!("{failure}\n    {change}"),
                        _ => format!("{inside}{failure}\n    {change}"),
                    };
                    let (caught, after) = if reader == "the catch" { (format!("\n    return {read}\n  "), "\"done\"".to_string()) } else { ("\n  ".to_string(), read.to_string()) };
                    let body = format!("{outside}try {{\n    {block}\n  }} catch {{{caught}}}\n  return {after}");
                    out.push((format!("{kind}, changed {place} the failing line, read by {reader}, failing by {failure}"), body));
                }
            }
        }
    }
    out
}

/// What JavaScript does with each handler on each request: its answer, then what it left in
/// `state.k`, which items of `notes` it put and of `gone` it deleted.
fn join_in_javascript(handlers: &[(String, String)]) -> Vec<Vec<String>> {
    let made: Vec<String> = handlers.iter().map(|(_, body)| format!("({{ state, notes, gone }}) => (req) => {{\n{body}\n}}")).collect();
    let source = format!(
        r#"(requests) => {{
  const handlers = [
{}
  ];
  const run = (make, req) => {{
    const fx = [];
    const state = {{}};
    const notes = new Proxy({{}}, {{ set: (t, k, v) => {{ fx.push("puts notes " + k + " = " + v); return true }} }});
    const gone = new Proxy({{}}, {{ deleteProperty: (t, k) => {{ fx.push("deletes gone " + k); return true }} }});
    const answer = make({{ state, notes, gone }})(req);
    if (state.k !== undefined) fx.push("writes k = " + state.k);
    return ["answers " + answer, ...fx].join("; ");
  }};
  return handlers.map((make) => requests.map((req) => run(make, req)));
}}"#,
        made.join(",\n")
    );
    let requests: Vec<Value> = JOIN_REQUESTS
        .iter()
        .map(|body| Value::Record(BTreeMap::from([("body".to_string(), Value::Text(body.to_string())), ("json".to_string(), holon_core::parse_json(body).unwrap())])))
        .collect();
    let limits = holon_core::ModuleLimits { fuel: 1_000_000_000, memory: 64 << 20 };
    let outs = holon_module::shared().run(holon_module::JS_ENGINE, &limits, &[Some(Value::Text(source)), Some(Value::List(vec![Value::List(requests)]))]).unwrap();
    let Some(Value::Record(answer)) = outs.into_iter().next().flatten() else { panic!("the engine answers a record") };
    let Some(Value::List(per_handler)) = answer.get("value") else { panic!("JavaScript failed: {answer:?}") };
    let list = |v: &Value| match v {
        Value::List(items) => items.clone(),
        other => panic!("{other:?}"),
    };
    per_handler.iter().map(|per| list(per).iter().map(|v| if let Value::Text(t) = v { t.clone() } else { panic!("{v:?}") }).collect()).collect()
}

/// **Guards:** a `try` block keeps what it did above the line that failed, and the catch and the
/// code after the try see the last value each thing it changed was given, as JavaScript's do: for
/// a local name, a state key (and one written on both sides of the failing line, which keeps the
/// first), a collection item put and one deleted; changed before or after the failing line; read
/// by the catch or by what follows the try; for each way a line fails (a text that isn't JSON, a
/// field read through one that's missing, a throw), on a request where it fails and one where it
/// doesn't.
///
/// **Why:** C7 (2026-11-25) made a try JavaScript's: each line that may fail is a branch on what
/// it made arriving, joined where the block ends as an `if`'s branches are (ts.rs
/// `join_arrivals`). One join for every kind, so one sweep holds them all.
///
/// **If it fails:** the line names the handler, the request, and what JavaScript and the flow did.
#[test]
fn a_try_keeps_the_last_value_reached_as_javascript_does() {
    let handlers = join_handlers();
    let javascript = join_in_javascript(&handlers);
    let state = claims_state("");
    let mut text = "import { flow, route } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\nimport gone from \"collection:gone\"\n\nflow(\"try-join\", { port: 19643 })\n".to_string();
    for (i, (_, body)) in handlers.iter().enumerate() {
        text.push_str(&format!("\nroute.post(\"/j{i}\", (req) => {{\n  {body}\n}})\n"));
    }
    if let Err(e) = holon_author::lang::compile(&text) {
        panic!("the sweep's flow is refused: {e}\n{text}");
    }
    let slug = deploy_plain_text(&state, &text);
    let (mut same, mut wrong) = (0, Vec::new());
    for (i, ((name, _), said)) in handlers.iter().zip(&javascript).enumerate() {
        for (request, js) in JOIN_REQUESTS.iter().zip(said) {
            let before = log_entries(&state, &slug).iter().filter_map(|e| e["seq"].as_u64()).max();
            let (status, answer, _) = data_call(&state, &slug, "POST", &format!("/j{i}"), request);
            let entries = log_entries(&state, &slug);
            let ran = entries.iter().find(|e| before.is_none_or(|b| e["seq"].as_u64().is_some_and(|s| s > b))).expect("the run's entry");
            let mut did = vec![if status == 200 { format!("answers {}", answer.trim_matches('"')) } else { format!("answers {status}") }];
            let mut wrote = None;
            for p in ran["performed"].as_array().into_iter().flatten() {
                match p["capability"].as_str().unwrap_or("") {
                    "StateWrite(\"k\")" => wrote = Some(format!("writes k = {}", shown(&p["payload"]).trim_matches('"'))),
                    "CollectionWrite(\"notes\")" => did.push("puts notes k = one".to_string()),
                    "CollectionWrite(\"gone\")" => did.push("deletes gone k".to_string()),
                    other => panic!("an effect the sweep doesn't know: {other}"),
                }
            }
            did.extend(wrote);
            let did = did.join("; ");
            if did == *js {
                same += 1;
            } else {
                wrong.push(format!("{name} | {request} | JavaScript {js:?}, the flow {did:?}"));
            }
        }
    }
    println!("try join: {same} runs as JavaScript, {} not", wrong.len());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **Guards:** a loop is refused, in plain words, as the language says (docs/LANGUAGE.md: until
/// the L4's slice 6 a loop is refused): the reached rule has no loop to reach into.
#[test]
fn a_loop_is_refused_in_plain_words() {
    for body in ["for (const x of [1, 2]) {\n    ++state.n\n  }", "let i = 0\n  while (i < 2) {\n    i = i + 1\n  }"] {
        let text = format!("import {{ flow, route }} from \"holon\"\nimport state from \"state\"\n\nflow(\"looping\", {{ port: 19641 }})\n\nroute.post(\"/go\", (req) => {{\n  {body}\n  return \"end\"\n}})\n");
        let refused = holon_author::lang::ts::compile(&text).map(|_| ()).expect_err("a loop is refused").to_string();
        println!("{refused}");
        assert!(refused.contains("loop") || refused.contains("map"), "{refused}");
    }
}

/// The columns a difference of reached is shown on (step C1). `Reached` is read off a site's run: a
/// step made where its path says it isn't reached (or where a test of its path was never looked
/// at), missed where it is, or run past a line that gave nothing. (`Kept`, what a failed `try`
/// kept, went with C7: a try keeps what it did, as JavaScript's does.) `ListOrder` (what a function a list method calls per item does, and in which order) is
/// its own column, not read by site runs. `Value` is what a
/// value is, not where a step is reached: its rows show no site difference at all.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Column {
    Reached,
    ListOrder,
    Value,
}

/// Each debt's column.
const COLUMNS: &[(&str, Column)] = &[
    (A_CHOICES_TEST_IS_READ_PAST_A_RETURN.name, Column::Reached),
    (A_READ_IS_ASKED_PAST_A_RETURN.name, Column::Reached),
    (A_LINE_AFTER_A_FAILURE_RUNS.name, Column::Reached),
    (AN_UNUSED_READ_IS_NOT_ASKED.name, Column::Reached),
    (A_LISTS_FUNCTION_GOES_PAST_A_FAILURE.name, Column::ListOrder),
    (A_REVIVERS_NOTHING_IS_NULL.name, Column::ListOrder),
    (A_FOLD_STOPS_AT_NOTHING.name, Column::ListOrder),
    (A_LIST_WITH_A_MISSING_ITEM_IS_NOTHING.name, Column::Value),
];

/// The rows whose difference site runs don't read yet, by name, and why. The list only shrinks:
/// a row on it that shows its difference fails, to be taken off.
const NOT_READ_YET: &[(&str, &str)] = &[
    ("a list's function that starts something and then fails, on the first item", "list order"),
    ("a reviver that fails on an item, in a try", "list order: a function JSON.parse calls per item"),
    ("a reduce that adds a start's handle to a text", "list order"),
];

/// What a text's sites do on a body sent to each route (or socket message) handler, beside where
/// their paths say they are reached: per column, each site run that differs, as `<what it does>
/// at line <n>: <verdict>`.
pub(super) fn site_differences(text: &str, body: &str) -> Result<Vec<String>, String> {
    use holon_author::reached_runs::{answer_ok, given_on, site_runs, Handler, Watched};
    let w = Watched::of(text)?;
    let given = |h: &Handler, u: &holon_core::Unit| if matches!(h.kind, "route" | "websocket") { given_on(h, u, &[body.to_string()]) } else { Vec::new() };
    let mut reached = Vec::new();
    for r in site_runs(&w, &given, answer_ok) {
        let site = &w.lowered.sites[r.site];
        let verdict = r.verdict(site);
        let at = format!("{} at line {}", site.does, site.at.start.line);
        if is_owed(verdict) {
            reached.push(format!("{at}: {verdict:?}"));
        }
    }
    Ok(reached)
}

/// Whether a site run's verdict is a difference owed: made where not reached or not looked at,
/// missed where reached, or run past a line that gave nothing while that is owed (the one switch,
/// `PAST_NOTHING`).
pub(super) fn is_owed(verdict: holon_author::reached_runs::Verdict) -> bool {
    use holon_author::reached_runs::{PastNothing, Verdict};
    matches!(verdict, Verdict::MadeUnreached | Verdict::MissedReached | Verdict::MadeUnlooked | Verdict::PastNothing(PastNothing::Owed))
}

/// **Guards:** reached (step C1) is read off every site of every sample and every row of the
/// table, beside what the flow does today: a site that differs from its path is owed under a named
/// debt, in that debt's column, and nowhere else.
/// - A sample's site run that differs fails, but one past a line that gave nothing, owed under
///   its debt while the switch says owed.
/// - A row as JavaScript shows nothing; a debt's row shows its difference in its column, and none
///   in another; a row whose column site runs don't read yet is listed (`NOT_READ_YET`), and
///   fails once it shows, to be taken off.
///
/// **If it fails:** read the line it names. A path the Reader got wrong is fixed in `lang/ts.rs`;
/// a graph doing other than its text is a new hole, a named row here, never a patch.
#[test]
fn every_site_is_reached_as_its_row_says() {
    use holon_author::reached_runs::{given, site_runs, answer_ok, Verdict, Watched};
    let mut wrong = Vec::new();
    // Every sample, on golden's bodies.
    let mut past_nothing = 0;
    for s in holon_author::samples::samples() {
        let w = Watched::of(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name));
        for r in site_runs(&w, &given, answer_ok) {
            let site = &w.lowered.sites[r.site];
            match r.verdict(site) {
                Verdict::PastNothing(_) => past_nothing += 1,
                v if is_owed(v) => wrong.push(format!("sample {} | {} | {} at line {}: {v:?}, under no row", s.name, r.handler, site.does, site.at.start.line)),
                _ => {}
            }
        }
    }
    println!("samples: {past_nothing} site runs past a line that gave nothing (\"{}\")", A_LINE_AFTER_A_FAILURE_RUNS.name);
    // Every row of the table, on its request.
    let names: Vec<&str> = rows().iter().map(|r| r.name).collect();
    for (name, _) in NOT_READ_YET {
        assert!(names.contains(name), "NOT_READ_YET names {name:?}, which is no row");
    }
    for (debt, _) in COLUMNS {
        assert!(DEBTS.iter().any(|d| d.name == *debt), "COLUMNS names {debt:?}, which is no debt");
    }
    for debt in DEBTS {
        assert!(COLUMNS.iter().any(|(d, _)| *d == debt.name), "{:?} has no column", debt.name);
    }
    for row in rows() {
        let text = row.text.replace("@PING@", PING).replace("@REFUSED@", "127.0.0.1:9");
        let reached = match site_differences(&text, row.request) {
            Ok(d) => d,
            Err(e) => {
                wrong.push(format!("{}: not read: {e}", row.name));
                continue;
            }
        };
        let shown = |c: Column| match c {
            Column::Reached => reached.clone(),
            Column::ListOrder | Column::Value => Vec::new(),
        };
        let all: Vec<String> = reached.clone();
        let not_yet = NOT_READ_YET.iter().find(|(n, _)| *n == row.name);
        match (&row.today, not_yet) {
            // What its sites say waits on the owner's answer, as the row does.
            (Today::WaitsForTheOwner(..), _) => {}
            (Today::AsJavascript, _) => {
                if !all.is_empty() {
                    wrong.push(format!("{} reads as JavaScript, its sites differ: {}", row.name, all.join("; ")));
                }
            }
            (_, Some((_, why))) => {
                if !all.is_empty() {
                    wrong.push(format!("{} shows now ({}): take it off NOT_READ_YET ({why})", row.name, all.join("; ")));
                }
            }
            (Today::Differs(debt, _), None) => {
                let column = COLUMNS.iter().find(|(d, _)| *d == debt.name).map(|(_, c)| *c).expect("a column");
                // A value's difference is no reached one: its sites read as their paths say.
                if column == Column::Value {
                    if !all.is_empty() {
                        wrong.push(format!("{} differs in what a value is, its sites too: {}", row.name, all.join("; ")));
                    }
                    continue;
                }
                if shown(column).is_empty() {
                    wrong.push(format!("{} shows nothing in its column, {column:?} (\"{}\")", row.name, debt.name));
                }
                let elsewhere: Vec<String> = [Column::Reached].into_iter().filter(|c| *c != column).flat_map(shown).collect();
                if !elsewhere.is_empty() {
                    wrong.push(format!("{} differs outside its column {column:?}: {}", row.name, elsewhere.join("; ")));
                }
                println!("{} [{}]: {}", row.name, debt.name, shown(column).join("; "));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
