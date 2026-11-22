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
//! - a row whose text the round trip refuses (`lang::compile`) is deployed by its hash with the
//!   reader alone (`deploy_read_alone`), so what it does stays on record until the printer and
//!   the rule take it.
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

/// F: a function's own condition stands in for its entry.
pub(super) const A_FUNCTIONS_OWN_CONDITION: Debt = Debt {
    name: "a function's own condition, not its entry",
    says: "a function called under an if that doesn't hold still does what its own if allows: its start goes out",
};
/// S: a failed try keeps what its functions made.
pub(super) const A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE: Debt = Debt {
    name: "a failed try keeps what its functions made",
    says: "a write a function makes in a try block that then fails is kept, where the try's promise keeps nothing",
};
/// L: what a list's function makes is dropped.
pub(super) const A_LISTS_FUNCTION_LOSES_ITS_WRITE: Debt = Debt {
    name: "what a list's function makes is dropped",
    says: "a write made by a function given to a list method is lost, where JavaScript makes it once per item",
};
/// R173: a list's function starts where the run doesn't go.
pub(super) const A_LISTS_FUNCTION_STARTS_PAST_A_RETURN: Debt = Debt {
    name: "a list's function starts where the run doesn't go",
    says: "an action a list's function starts goes out below a return that answered",
};
/// E: a call in a choice inside a function takes no trigger.
pub(super) const A_CHOICE_IN_A_FUNCTION_STARTS: Debt = Debt {
    name: "a call in a choice inside a function takes no trigger",
    says: "a function whose start sits in a ?: arm starts it though the function's call is under an if that doesn't hold",
};
/// W: a finally forgets the returns above it.
pub(super) const A_FINALLY_FORGETS_THE_RETURN_ABOVE: Debt = Debt {
    name: "a finally forgets the returns above",
    says: "a state write in a finally is made though a return above the try answered and the try was never reached",
};
/// P+R: a wait past a return that answered still waits.
pub(super) const A_WAIT_PAST_AN_ANSWER_PARKS: Debt = Debt {
    name: "a wait past a return that answered still waits",
    says: "an action that returned above its wait parks at the wait anyway, and its caller is answered a waiting link",
};
/// A: an arm's test is read before its statement is reached.
pub(super) const A_CHOICES_TEST_IS_READ_PAST_A_RETURN: Debt = Debt {
    name: "the arm's test is read before the statement's reached",
    says: "a ?: whose test reads a collection asks that read below a return that answered",
};
/// Z: a call in an arm, read only in a branch not taken, isn't made.
pub(super) const AN_ARMS_CALL_READ_ONLY_ELSEWHERE_IS_NOT_MADE: Debt = Debt {
    name: "a call in an arm, read only in a branch not taken, isn't made",
    says: "an action started in a ?: arm that is taken is not started when its value is read only in a branch that isn't",
};
/// R: a read is asked where its statement isn't reached.
pub(super) const A_READ_IS_ASKED_PAST_A_RETURN: Debt = Debt {
    name: "a read is asked where its statement isn't reached",
    says: "a collection read in a send's content is asked below a return that answered, though the send is held",
};
/// B: a line after a failure outside a try still runs. The owner's question, rule or debt: a debt
/// until it is decided.
pub(super) const A_LINE_AFTER_A_FAILURE_RUNS: Debt = Debt {
    name: "a line after a failure outside a try still runs",
    says: "a write on the line after a JSON.parse that fails, with no try around them, is made, where JavaScript throws first",
};

/// Every debt of the table, each to be struck when no row differs under it.
pub(super) const DEBTS: &[&Debt] = &[
    &A_FUNCTIONS_OWN_CONDITION,
    &A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE,
    &A_LISTS_FUNCTION_LOSES_ITS_WRITE,
    &A_LISTS_FUNCTION_STARTS_PAST_A_RETURN,
    &A_CHOICE_IN_A_FUNCTION_STARTS,
    &A_FINALLY_FORGETS_THE_RETURN_ABOVE,
    &A_WAIT_PAST_AN_ANSWER_PARKS,
    &A_CHOICES_TEST_IS_READ_PAST_A_RETURN,
    &AN_ARMS_CALL_READ_ONLY_ELSEWHERE_IS_NOT_MADE,
    &A_READ_IS_ASKED_PAST_A_RETURN,
    &A_LINE_AFTER_A_FAILURE_RUNS,
];

/// How a row stands today.
enum Today {
    /// The flow does what JavaScript does.
    AsJavascript,
    /// The flow is taken and does these marks instead, under the debt.
    Differs(&'static Debt, &'static str),
    /// `lang::compile` refuses the text (the round trip); deployed by its hash with the reader
    /// alone, it does these marks, under the debt.
    Refused(&'static Debt, &'static str),
}

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
            today: Today::Refused(&A_FUNCTIONS_OWN_CONDITION, "answers \"end\"; starts ping; writes pinged = 1"),
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
            // Holon's promise, not JavaScript's: a try block that fails keeps nothing.
            javascript: "answers \"bad\"",
            today: Today::Differs(&A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE, "answers \"bad\"; writes n = 1"),
            long: Some((
                "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-02-long\", { port: 19622 })\n\nroute.post(\"/go\", (req) => {\n  try {\n    ++state.n\n    const parsed = JSON.parse(req.json.text)\n  } catch {\n    return \"bad\"\n  }\n  return \"end\"\n})\n",
                "answers \"bad\"",
            )),
        },
        Row {
            name: "a function that writes, given to a list's map",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-03\", { port: 19603 })\n\nfunction keep(x) {\n  ++state.n\n  return x + \"!\"\n}\n\nroute.post(\"/go\", (req) => {\n  const got = [req.json.a].map((x) => keep(x))\n  return \"end\"\n})\n",
            request: r#"{"a":"x"}"#,
            answer: true,
            javascript: "answers \"end\"; writes n = 1",
            today: Today::Refused(&A_LISTS_FUNCTION_LOSES_ITS_WRITE, "answers \"end\""),
            long: None,
        },
        Row {
            name: "a list's function that starts something, below a return that answered",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-04\", { port: 19604 })\n\n@PING@\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"early\"\n  const got = [req.json.a].map((x) => ping(x))\n  return \"end\"\n})\n",
            request: r#"{"stop":true,"a":"x"}"#,
            answer: true,
            javascript: "answers \"early\"",
            today: Today::Refused(&A_LISTS_FUNCTION_STARTS_PAST_A_RETURN, "answers \"early\"; starts ping; writes pinged = 1"),
            long: None,
        },
        Row {
            name: "a function whose start is in a choice, called under an if that doesn't hold",
            text: "import { action, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-05\", { port: 19605 })\n\n@PING@\nfunction maybe(x) {\n  return x ? ping(x) : 0\n}\n\nroute.post(\"/go\", (req) => {\n  if (req.json.go) {\n    const made = maybe(req.json.a)\n  }\n  return \"end\"\n})\n",
            request: r#"{"go":false,"a":"x"}"#,
            answer: true,
            javascript: "answers \"end\"",
            today: Today::Refused(&A_CHOICE_IN_A_FUNCTION_STARTS, "answers \"end\"; starts ping; writes pinged = 1"),
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
            today: Today::Differs(&A_FINALLY_FORGETS_THE_RETURN_ABOVE, "answers \"stopped\"; writes fin = 1"),
            long: Some((
                "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-06-long\", { port: 19626 })\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  if (!req.json.stop) {\n    try {\n      const parsed = JSON.parse(req.json.text)\n    } catch {\n      return \"bad\"\n    } finally {\n      ++state.fin\n    }\n  }\n  return \"end\"\n})\n",
                "answers \"stopped\"",
            )),
        },
        Row {
            name: "an action that answers above its wait",
            text: "import { action, event, flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"reached-07\", { port: 19607 })\n\nexport const pay = action(async (input) => {\n  if (input.stop) return \"stopped\"\n  const ok = await event(\"approved\", { expires: \"1h\" })\n  updates.send({ status: \"paid\", by: ok.by })\n  return \"done\"\n})\n\nroute.post(\"/go\", (req) => {\n  const started = pay(req.json)\n  return started\n})\n",
            request: r#"{"stop":true}"#,
            answer: true,
            javascript: "answers \"stopped\"; starts pay",
            today: Today::Differs(&A_WAIT_PAST_AN_ANSWER_PARKS, "answers a waiting link; starts pay; parks"),
            long: Some((
                "import { action, event, flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"reached-07-long\", { port: 19627 })\n\nexport const pay = action(async (input) => {\n  const ok = await event(\"approved\", { expires: \"1h\" })\n  updates.send({ status: \"paid\", by: ok.by })\n  return \"done\"\n})\n\nroute.post(\"/go\", (req) => {\n  if (req.json.stop) return \"stopped\"\n  const started = pay(req.json)\n  return started\n})\n",
                "answers \"stopped\"",
            )),
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
            today: Today::Differs(&AN_ARMS_CALL_READ_ONLY_ELSEWHERE_IS_NOT_MADE, "answers \"no\""),
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
            name: "a key written, then written again in an if on a field that's missing",
            text: "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"reached-12\", { port: 19612 })\n\nroute.post(\"/go\", (req) => {\n  state.k = \"two\"\n  if (req.json.missing) {\n    state.k = \"one\"\n  }\n  return \"end\"\n})\n",
            request: r#"{}"#,
            answer: true,
            javascript: "answers \"end\"; writes k = \"two\"",
            today: Today::AsJavascript,
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

/// A row's text deployed: by the language when `lang::compile` takes it, by its hash with the
/// reader alone when the round trip refuses it.
fn deployed(state: &AppState, text: &str, host: &str, refused: bool) -> String {
    if refused { deploy_read_alone(state, text, host) } else { deploy_text(state, text, host) }
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
    let mut wrong = Vec::new();
    let mut owing: BTreeSet<&str> = BTreeSet::new();
    for row in rows() {
        let text = row.text.replace("@PING@", PING);
        let refused = holon_author::lang::compile(&text).is_err();
        let slug = deployed(&state, &text, &host, refused);
        let did = marks_of(&state, &slug, "/go", row.request, row.answer);
        match row.today {
            Today::AsJavascript => {
                assert!(!refused, "{}: the round trip refuses it: {:?}", row.name, holon_author::lang::compile(&text).err());
                if did != row.javascript {
                    wrong.push(format!("{}: JavaScript {:?}, the flow {did:?}", row.name, row.javascript));
                }
            }
            Today::Differs(debt, recorded) | Today::Refused(debt, recorded) => {
                let should_refuse = matches!(row.today, Today::Refused(..));
                assert_eq!(refused, should_refuse, "{}: the round trip {} it now: {:?}", row.name, if refused { "refuses" } else { "takes" }, holon_author::lang::compile(&text).err());
                owing.insert(debt.name);
                if did == row.javascript {
                    wrong.push(format!("\"{}\" no longer differs on {:?}: strike it ({})", debt.name, row.name, debt.says));
                } else if did != recorded {
                    wrong.push(format!("{}: differs another way than \"{}\" records: recorded {recorded:?}, the flow {did:?}, JavaScript {:?}", row.name, debt.name, row.javascript));
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
