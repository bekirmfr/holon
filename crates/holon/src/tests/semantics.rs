//! What a handler's text does, against what the same text does as JavaScript.
//!
//! A flow is written in TypeScript's syntax and is not run as TypeScript: it compiles to a graph,
//! where what is done or not done is a matter of which node's trigger is tied to what. Its reader
//! is a person approving it or an agent writing it, and both read it as the JavaScript it looks
//! like. So the question a test of the language has to ask is not "does this handler compile to
//! that graph" but "does this handler do what its text says", and what its text says is what
//! JavaScript does with it. JavaScript is here to be asked (the engine a `js` node runs on,
//! holon-module), so it is: the same handler body is run both ways, over a grid, and what each
//! did is compared.
//!
//! The grid is every way a statement can be reached or not (`SHAPES`), crossed with every kind of
//! thing a handler does (`EFFECTS`), every form of condition (`CONDITIONS`) and requests that make
//! each condition hold, not hold, and have nothing to go on (`REQUESTS`). A handler the language
//! refuses is left out: a refusal says so at `holon check`, which is safe.
//!
//! A flow differs from JavaScript by one rule, on purpose: it has no `undefined` to compute with
//! and nothing to throw, so a value that isn't there makes what is computed from it not there
//! either, and a condition that isn't there doesn't hold (docs/LANGUAGE.md §5.6). That rule is
//! written here once, as JavaScript (`AS_A_FLOW_READS`), and every condition has the form it takes
//! under it. So each handler is run three ways: as the flow, as JavaScript, and as JavaScript with
//! that rule. The flow does what the third does, always; where the third differs from the second
//! is the whole of what a reader of JavaScript has to know, and is counted.
//!
//! To add a case: a new way to reach a statement is a line of `SHAPES`, a new kind of effect a line
//! of `EFFECTS` (and its mark where the log is read, `flow_does`), a new condition a line of
//! `CONDITIONS`, a new request a line of `REQUESTS`. None needs its answer written.

use super::*;

/// The kinds of thing a handler does: the mark each leaves, the statement that does it, and the
/// same as an expression whose value is used. `rec` is a function that writes, `ask` one that
/// calls out, `ping` an action of the flow's. A function places functions, so each is also reached
/// through others that return none of what it does (`rec2`, `rec3`, `ask2`) or only a part of it
/// (`part`, which returns `both`'s put and not its write): what a function does, the one placing
/// it does.
const EFFECTS: &[(&str, &str, &str)] = &[
    ("w", "++state.w", ""),
    ("p", "marks.put(\"k\", \"v\")", ""),
    ("l", "console.log(\"line\")", ""),
    ("s", "ping({})", "ping({})"),
    ("f", "rec(\"a\")", "rec(\"a\")"),
    ("r", "const sent = await api.post(\"/hit\", { body: \"x\" })", "await api.post(\"/hit\", { body: \"x\" })"),
    ("g", "const asked = await ask(\"a\")", "await ask(\"a\")"),
    ("f2", "rec2(\"a\")", "rec2(\"a\")"),
    ("f3", "rec3(\"a\")", "rec3(\"a\")"),
    ("fp", "part(\"a\")", "part(\"a\").did"),
    ("g2", "const asked = await ask2(\"a\")", "await ask2(\"a\")"),
];

/// The forms a condition takes, over fields a request may lack: as written, and as a flow reads
/// it (`AS_A_FLOW_READS`).
const CONDITIONS: &[(&str, &str)] = &[
    ("req.json.c", "J.c"),
    ("!req.json.c", "N(J.c)"),
    ("req.json.n > 10", "GT(J.n, 10)"),
    ("req.json.n <= 10", "LE(J.n, 10)"),
    ("req.json.s === \"x\"", "EQ(J.s, \"x\")"),
    ("req.json.s !== \"x\"", "NE(J.s, \"x\")"),
    ("req.json.c && req.json.d", "AND(J.c, J.d)"),
    ("req.json.c || req.json.d", "OR(J.c, J.d)"),
    ("!(req.json.n > 10)", "N(GT(J.n, 10))"),
];

/// How a flow reads what JavaScript computes with `undefined`, as JavaScript: nothing in, nothing
/// out. `!` of nothing is nothing; a comparison takes two numbers or two texts, and anything else
/// gives nothing; equality of nothing is nothing; `&&` and `||` of nothing on either side are
/// nothing, whatever the other side is (`true || nothing` is nothing, where JavaScript's is true).
const AS_A_FLOW_READS: &str = r#"
    const N = (x) => x === undefined ? undefined : !x;
    const alike = (a, b) => typeof a === typeof b && (typeof a === "number" || typeof a === "string");
    const GT = (a, b) => alike(a, b) ? a > b : undefined;
    const LE = (a, b) => alike(a, b) ? a <= b : undefined;
    const EQ = (a, b) => a === undefined || b === undefined ? undefined : a === b;
    const NE = (a, b) => a === undefined || b === undefined ? undefined : a !== b;
    const AND = (a, b) => a === undefined || b === undefined ? undefined : !!a && !!b;
    const OR = (a, b) => a === undefined || b === undefined ? undefined : !!a || !!b;
"#;

/// Requests' bodies: fields that hold, that don't, that are missing, of another type, and a body
/// that isn't JSON at all.
const REQUESTS: &[&str] = &[
    r#"{"c":true,"d":true,"n":50,"s":"x","k":"a","v":"kept"}"#,
    r#"{"c":false,"d":false,"n":5,"s":"y","k":"b"}"#,
    r#"{}"#,
    r#"{"c":"yes","d":0,"n":"50","s":1,"k":"z"}"#,
    r#"{"c":0,"d":"","n":10,"s":""}"#,
    r#"{"c":true}"#,
    r#"{"d":true,"n":11,"s":"x"}"#,
    "not json",
];

/// The ways a statement is reached or not: a name, and a handler's body with `{C}` a condition,
/// `{E}` the effect as a statement and `{V}` as a value. `++state.o` is another effect, where a
/// shape needs two.
const SHAPES: &[(&str, &str)] = &[
    ("always", "{E}\nreturn \"end\""),
    ("if", "if ({C}) {\n{E}\n}\nreturn \"end\""),
    ("if else", "if ({C}) {\n{E}\n} else {\n++state.o\n}\nreturn \"end\""),
    ("else", "if ({C}) {\n++state.o\n} else {\n{E}\n}\nreturn \"end\""),
    ("else if", "if ({C}) {\n++state.o\n} else if (req.json.d) {\n{E}\n}\nreturn \"end\""),
    ("nested if", "if ({C}) {\nif (req.json.d) {\n{E}\n}\n}\nreturn \"end\""),
    ("after an if", "if ({C}) {\n++state.o\n}\n{E}\nreturn \"end\""),
    ("after a return", "if ({C}) return \"stopped\"\n{E}\nreturn \"end\""),
    ("after a return in a block", "if ({C}) {\nreturn status(403)\n}\n{E}\nreturn \"end\""),
    ("after two returns", "if ({C}) return \"a\"\nif (req.json.d) return \"b\"\n{E}\nreturn \"end\""),
    ("after a return in an if", "if ({C}) {\nif (req.json.d) return \"stopped\"\n{E}\n}\nreturn \"end\""),
    ("before a return", "{E}\nif ({C}) return \"stopped\"\nreturn \"end\""),
    ("switch", "switch (req.json.k) {\ncase \"a\":\n{E}\nbreak\ncase \"b\":\n++state.o\nbreak\ndefault:\nbreak\n}\nreturn \"end\""),
    ("switch default", "switch (req.json.k) {\ncase \"a\":\n++state.o\nbreak\ndefault:\n{E}\nbreak\n}\nreturn \"end\""),
    ("a branch of ?:", "const got = {C} ? {V} : undefined\nreturn \"end\""),
    ("the other branch of ?:", "const got = {C} ? \"no\" : {V}\nreturn \"end\""),
    ("after &&", "const got = {C} && {V}\nreturn \"end\""),
    ("after ||", "const got = {C} || {V}\nreturn \"end\""),
    ("after ??", "const got = req.json.v ?? {V}\nreturn \"end\""),
    ("a branch of ?: in an if", "if (req.json.d) {\nconst got = {C} ? {V} : undefined\n}\nreturn \"end\""),
    ("bound, and read in a branch", "const got = {V}\nreturn {C} ? got : \"no\""),
    ("tied to a value there", "let got\nif (req.json.v !== undefined) got = {V}\nreturn \"end\""),
    ("returned from a case", "switch (req.json.k) {\ncase \"a\":\nreturn {V}\ndefault:\nreturn \"no\"\n}"),
    ("in a try", "try {\nconst parsed = JSON.parse(req.body)\n{E}\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
];

/// A handler of the grid: where it comes from, its body, and its body as a flow reads it.
#[derive(Clone)]
struct Case {
    shape: &'static str,
    mark: &'static str,
    condition: &'static str,
    body: String,
    read: String,
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (shape, template) in SHAPES {
        for (mark, statement, value) in EFFECTS {
            if template.contains("{V}") && value.is_empty() {
                continue;
            }
            // A shape with no condition is one case, not one a condition.
            let conditions = if template.contains("{C}") { CONDITIONS } else { &CONDITIONS[..1] };
            for (condition, read) in conditions {
                let fill = |c: &str| template.replace("{C}", c).replace("{E}", statement).replace("{V}", value);
                // A subject that isn't there takes no case, the default no more than another.
                let read = fill(read).replace("req.json.", "J.").replace("switch (J.k) {", "if (J.k !== undefined) switch (J.k) {");
                out.push(Case { shape, mark, condition, body: fill(condition), read });
            }
        }
    }
    out
}

/// A flow of `handlers`, each a route `POST /r<i>`: only what they use is imported and defined.
fn flow_text(name: &str, port: u16, host: &str, handlers: &[(usize, &Case)]) -> String {
    let uses = |what: &str| handlers.iter().any(|(_, c)| c.body.contains(what));
    let mut text = format!("import {{ {}flow, route{} }} from \"holon\"\nimport state from \"state\"\n", if uses("ping(") { "action, " } else { "" }, if uses("status(") { ", status" } else { "" });
    if uses("marks.") || uses("part(") {
        text.push_str("import marks from \"collection:marks\"\n");
    }
    if uses("api.") || uses("ask(") || uses("ask2(") {
        text.push_str(&format!("import api from \"host:{host}\"\n"));
    }
    text.push_str(&format!("\nflow(\"{name}\", {{ port: {port} }})\n"));
    if uses("rec(") || uses("rec2(") || uses("rec3(") {
        text.push_str("\nfunction rec(x) {\n  ++state.f\n  return `did ${x}`\n}\n");
    }
    if uses("rec2(") || uses("rec3(") {
        text.push_str("\nfunction rec2(x) {\n  const did = rec(x)\n  return `${did} twice`\n}\n");
    }
    if uses("rec3(") {
        text.push_str("\nfunction rec3(x) {\n  const did = rec2(x)\n  return `${did}, thrice`\n}\n");
    }
    if uses("part(") {
        text.push_str("\nfunction both(x) {\n  ++state.f\n  const put = marks.put(\"k\", x)\n  const did = `did ${x}`\n  return { put, did }\n}\n");
        text.push_str("\nfunction part(x) {\n  const done = both(x)\n  return { put: done.put, did: done.did }\n}\n");
    }
    if uses("ask(") || uses("ask2(") {
        text.push_str("\nasync function ask(x) {\n  const sent = await api.post(\"/in\", { body: x })\n  return sent.status\n}\n");
    }
    if uses("ask2(") {
        text.push_str("\nasync function ask2(x) {\n  const got = await ask(x)\n  return got ?? 0\n}\n");
    }
    if uses("ping(") {
        text.push_str("\nexport const ping = action(() => ++state.pinged)\n");
    }
    for (i, case) in handlers {
        let r#async = if case.body.contains("await ") { "async " } else { "" };
        text.push_str(&format!("\nroute.post(\"/r{i}\", {async}(req) => {{\n{}\n}})\n", case.body));
    }
    text
}

/// What a way of running a handler did: the marks it left, in order of their letters, and
/// whether it threw.
#[derive(Clone, Debug, PartialEq)]
struct Did {
    marks: String,
    threw: bool,
}

/// What JavaScript does with each of `handlers` on each of `REQUESTS`, as written and as a flow
/// reads it. The names a handler uses are stand-ins that leave their mark; `await` is dropped,
/// since they answer at once.
fn javascript_does(handlers: &[&Case]) -> Vec<Vec<(Did, Did)>> {
    let names = "state, marks, console, ping, rec, rec2, rec3, part, ask, ask2, api, status";
    let made: Vec<String> = handlers
        .iter()
        .map(|c| {
            let (written, read) = (c.body.replace("await ", ""), c.read.replace("await ", ""));
            format!("[({{ {names} }}) => (req) => {{\n{written}\n}}, ({{ {names} }}) => (req) => {{\nconst J = req.json ?? {{}};\n{read}\n}}]")
        })
        .collect();
    let source = format!(
        r#"(requests) => {{
  {AS_A_FLOW_READS}
  const handlers = [
{}
  ];
  const run = (make, req) => {{
    const fx = [];
    const state = {{}};
    const env = {{
      state,
      marks: {{ put: () => {{ fx.push("p") }} }},
      console: {{ log: () => {{ fx.push("l") }} }},
      ping: () => {{ fx.push("s"); return {{ run: "r" }} }},
      rec: (x) => {{ fx.push("f"); return "did " + x }},
      rec2: (x) => {{ fx.push("f"); return "did " + x + " twice" }},
      rec3: (x) => {{ fx.push("f"); return "did " + x + " twice, thrice" }},
      part: (x) => {{ fx.push("f"); fx.push("p"); return {{ did: "did " + x }} }},
      ask: () => {{ fx.push("g"); return 200 }},
      ask2: () => {{ fx.push("g"); return 200 }},
      api: {{ post: () => {{ fx.push("r"); return {{ status: 200, body: "fast" }} }} }},
      status: (n) => ({{ status: n }}),
    }};
    let threw = false;
    try {{ make(env)(req) }} catch (e) {{ threw = true }}
    for (const k of Object.keys(state)) fx.push(k);
    return {{ marks: fx.sort().join(""), threw }};
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
    let limits = holon_core::ModuleLimits { fuel: 4_000_000_000, memory: 256 << 20 };
    let outs = holon_module::shared().run(holon_module::JS_ENGINE, &limits, &[Some(Value::Text(source)), Some(Value::List(vec![Value::List(requests)]))]).unwrap();
    let Some(Value::Record(answer)) = outs.into_iter().next().flatten() else { panic!("the engine answers a record") };
    let Some(Value::List(per_handler)) = answer.get("value") else { panic!("JavaScript failed: {answer:?}") };
    let did = |v: &Value| {
        let Value::Record(r) = v else { panic!("{v:?}") };
        let (Some(Value::Text(marks)), Some(Value::Boolean(threw))) = (r.get("marks"), r.get("threw")) else { panic!("{r:?}") };
        Did { marks: marks.clone(), threw: *threw }
    };
    let list = |v: &Value| -> Vec<Value> {
        let Value::List(items) = v else { panic!("{v:?}") };
        items.clone()
    };
    per_handler.iter().map(|per_request| list(per_request).iter().map(|both| (did(&list(both)[0]), did(&list(both)[1]))).collect()).collect()
}

/// The marks the run of `POST /r<i>` left, read from what the log gained: what the run performed,
/// the requests it sent, and the runs of `ping` it started.
fn flow_does(state: &AppState, slug: &str, i: usize, body: &str, seen: &mut Option<u64>) -> String {
    data_call(state, slug, "POST", &format!("/r{i}"), body);
    let after = seen.map(|seq| format!("after={seq}&")).unwrap_or_default();
    let gained = body_json(route(state, &req("GET", &format!("/deployments/{slug}/log?{after}limit=50"), vec![])));
    let entries = gained["entries"].as_array().unwrap();
    let mut marks: Vec<&str> = Vec::new();
    for e in entries {
        *seen = (*seen).max(e["seq"].as_u64());
        if e["route"] == "action ping" {
            marks.push("s");
        }
        if e["kind"] != "request" {
            continue;
        }
        for p in e["performed"].as_array().into_iter().flatten() {
            match p["capability"].as_str().unwrap_or("") {
                "StateWrite(\"w\")" => marks.push("w"),
                "StateWrite(\"o\")" => marks.push("o"),
                "StateWrite(\"f\")" => marks.push("f"),
                "CollectionWrite(\"marks\")" => marks.push("p"),
                other => panic!("an effect the grid doesn't know: {other}"),
            }
        }
        let sent = serde_json::to_string(&e["rounds"]).unwrap();
        if sent.contains("/hit\"") {
            marks.push("r");
        }
        if sent.contains("/in\"") {
            marks.push("g");
        }
        // A line is no effect a run performed: it is kept beside the runs, under its run's seq.
        let lines = body_json(route(state, &req("GET", &format!("/deployments/{slug}/logs?run={}", e["seq"]), vec![])));
        if lines["lines"].as_array().is_some_and(|l| !l.is_empty()) {
            marks.push("l");
        }
    }
    marks.sort();
    marks.concat()
}

/// Where a flow doesn't yet do what it reads as, and why: debts, each to be paid by a change to
/// the language and then struck from here. A difference outside these fails the test, and so does
/// one of these that no longer differs anywhere.
const DEBTS: &[(&str, &str)] = &[
    ("try", "in a try block, a call bound after a statement that failed is still made, and a function after it still entered (docs/LANGUAGE.md §6.5a)"),
    ("and or", "`a && f()` and `a || f()` call f whatever a is: the right side isn't a branch"),
    ("a call in a function", "a request a function makes is sent when the function's answer is needed, not when it is called: bound and read only in a branch not taken, it isn't sent (docs/LANGUAGE.md F13)"),
];

fn debt(case: &Case, request: &str) -> Option<&'static str> {
    match case.shape {
        "in a try" if request == "not json" => Some("try"),
        "after &&" | "after ||" => Some("and or"),
        "bound, and read in a branch" if case.mark.starts_with('g') => Some("a call in a function"),
        _ => None,
    }
}

/// **Guards:** a handler does what its text says. What it writes, sends, starts and calls is what
/// JavaScript would with the same text, but for the one rule a flow has of its own, a value that
/// isn't there (docs/LANGUAGE.md §5.6), which is written out here and held exactly.
///
/// **Why:** a flow isn't run as the JavaScript it is written in. It compiles to a graph, where a
/// statement under an `if` or after a `return` is "not reached" only if the compiler tied that
/// statement's trigger to the condition, one kind of statement at a time. A kind the compiler
/// forgot runs on every path, and nothing else notices: the flow compiles, its permissions are the
/// same, and its answer is right.
///
/// **If it fails:** a handler does something on a path its text says it doesn't take: a payout
/// started after `return status(403)`, a write under an `if` that didn't hold. That was so for an
/// action started and for a function that writes or calls out, on every such path, until this test
/// (1,393 of 4,360 runs differed). Or, failing the other way, a handler doesn't do what it says.
#[test]
fn a_handler_does_what_its_text_says() {
    let state = claims_state("");
    let (host, _hits) = counting_server();
    let all = cases();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    // What the language refuses is left out: said at check, it is no surprise at a run.
    let indexed: Vec<(usize, &Case)> = all.iter().enumerate().collect();
    let taken: Vec<(usize, &Case)> = thread::scope(|s| {
        let checking: Vec<_> = indexed
            .chunks(indexed.len().div_ceil(workers))
            .map(|part| s.spawn(|| part.iter().filter(|(i, c)| holon_author::agent::plan(&flow_text("probe", 19000, &host, &[(*i, c)])).is_ok()).copied().collect::<Vec<_>>()))
            .collect();
        checking.into_iter().flat_map(|c| c.join().unwrap()).collect()
    });
    // A flow of forty handlers at a time, each flow's runs one after another (the log is read by
    // what it gained), the flows side by side.
    #[derive(Default)]
    struct Tally<'a> {
        runs: usize,
        as_javascript: usize,
        wrong: Vec<String>,
        owed: BTreeMap<&'static str, usize>,
        /// Where a flow's own rule shows: by the condition that had nothing to go on.
        shows: BTreeMap<&'a str, usize>,
    }
    let flows: Vec<(usize, &[(usize, &Case)])> = taken.chunks(40).enumerate().collect();
    let tallies: Vec<Tally> = thread::scope(|s| {
        let running: Vec<_> = flows
            .chunks(flows.len().div_ceil(workers))
            .map(|part| {
                let (state, host) = (&state, &host);
                s.spawn(move || {
                    let mut t = Tally::default();
                    for (n, chunk) in part {
                        let text = flow_text(&format!("grid-{n}"), 19001 + *n as u16, host, chunk);
                        let slug = if text.contains("host:") { deploy_text(state, &text, host) } else { deploy_plain_text(state, &text) };
                        let javascript = javascript_does(&chunk.iter().map(|(_, c)| *c).collect::<Vec<_>>());
                        let mut seen = None;
                        for ((i, case), said) in chunk.iter().zip(&javascript) {
                            for (request, (written, read)) in REQUESTS.iter().zip(said) {
                                let did = flow_does(state, &slug, *i, request, &mut seen);
                                t.runs += 1;
                                assert!(!read.threw, "as a flow reads it, nothing throws: {} | {} | {request}", case.shape, case.condition);
                                if did == written.marks && !written.threw {
                                    t.as_javascript += 1;
                                } else if did == read.marks {
                                    *t.shows.entry(case.condition).or_default() += 1;
                                }
                                if did == read.marks {
                                    continue;
                                }
                                match debt(case, request) {
                                    Some(owing) => *t.owed.entry(owing).or_default() += 1,
                                    None => t.wrong.push(format!(
                                        "{} | {} | {} | {request} | it reads as {:?} | the flow did {did:?} | javascript {:?}{}",
                                        case.shape,
                                        case.mark,
                                        case.condition,
                                        read.marks,
                                        written.marks,
                                        if written.threw { " (threw)" } else { "" }
                                    )),
                                }
                            }
                        }
                    }
                    t
                })
            })
            .collect();
        running.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let (mut runs, mut as_javascript, mut wrong) = (0, 0, Vec::new());
    let (mut owed, mut shows): (BTreeMap<&str, usize>, BTreeMap<&str, usize>) = Default::default();
    for t in tallies {
        runs += t.runs;
        as_javascript += t.as_javascript;
        wrong.extend(t.wrong);
        t.owed.into_iter().for_each(|(k, n)| *owed.entry(k).or_default() += n);
        t.shows.into_iter().for_each(|(k, n)| *shows.entry(k).or_default() += n);
    }
    println!("{} handlers, {} the language takes, {runs} runs: {as_javascript} as JavaScript, {} by a flow's own rule, {} owed", all.len(), taken.len(), shows.values().sum::<usize>(), owed.values().sum::<usize>());
    for (condition, n) in &shows {
        println!("  {n:4} runs where `{condition}` had nothing to go on");
    }
    assert!(wrong.is_empty(), "{} runs don't do what their text says:\n{}", wrong.len(), wrong.join("\n"));
    // The grid is no smaller than it was, and most of it is plain JavaScript.
    assert!(taken.len() > 600 && runs > 5_000, "{} handlers taken, {runs} runs", taken.len());
    assert!(as_javascript * 2 > runs, "{as_javascript} of {runs} runs do as JavaScript does");
    for (owing, why) in DEBTS {
        assert!(owed.contains_key(owing), "\"{owing}\" no longer differs anywhere: strike it from DEBTS ({why})");
    }
}
