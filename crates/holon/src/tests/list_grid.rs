//! What a list method's function does, item by item, against what the same text does as
//! JavaScript: the list block of reached.
//!
//! A list method's function (`xs.map((x) => send(x))`) compiles to a graph run once per item. What
//! a function placed in it makes is made once per item it reaches, in order, as JavaScript makes
//! it; and where the run doesn't go (past a return that answered, in a branch not taken, an `if`
//! or a function not entered, on the side of `&&`, `||` or `??` that isn't read), nothing. The
//! grid crosses the ways a list call is reached (`SHAPES`) with the methods that take a function
//! (`METHODS`, core's `PASSES_EXTRA` each reached by one), where the effect is written in the
//! function (`FORMS`) and its kind (`EFFECTS`), and runs each handler once, on one request, as
//! the flow and as JavaScript with the try's promise (a failed try block keeps nothing but the
//! requests it asked, docs/LANGUAGE.md §6.5) written as JavaScript, as the handler grid has it.
//!
//! What is compared is what each made, kind by kind, in the order made: each entry its kind and
//! the item it carries (`c:zz2`). Answers aren't compared: this is a grid of reached. The order
//! between kinds isn't either (a run's record keeps its requests, its writes and its starts
//! apart).
//!
//! The grid is the review session's list block's idea (its methods, forms and the try's promise),
//! written again for the reached branch, with section E's rows added: a list in a list's function,
//! in a catch, in a finally, and on both sides of `&&`, `||` and `??`. A wait is left out. Since C3
//! a list's function hands out what it makes (core's `did`) and its list call is tied as any step
//! that does something, so the grid compares what each made where the text says.

use super::reached::{Debt, A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE, A_FOLD_STOPS_AT_NOTHING, A_LISTS_FUNCTION_GOES_PAST_A_FAILURE, A_REVIVERS_NOTHING_IS_NULL};
use super::semantics::THE_STARTED_HALF;
use super::*;

/// The items a list holds, and a request holding them, `bad` no JSON for a try block to fail on.
const ITEMS: [&str; 3] = ["zz1", "zz2", "zz3"];
const LIST_REQUEST: &str = r#"{"a":"zz1","b":"zz2","c":"zz3","bad":"{"}"#;

/// The kinds of effect, each a statement on the item `x`: its mark and the statement. `r` asks a
/// request, kept by a try block that then fails, as JavaScript's is.
const EFFECTS: &[(&str, &str)] = &[
    ("w", "state.w = x"),
    ("p", "marks.put(x, x)"),
    ("c", "news.send({ n: x })"),
    ("q", "jobs.send({ data: { n: x } })"),
    ("m", "ops.send({ to: \"a@acme.com\", subject: x, text: \"hi\" })"),
    ("l", "console.log(x)"),
    ("s", "ping({ n: x })"),
    ("r", "const sent = await api.post(\"/in\", { body: x })"),
];

/// The methods that take a function: a name, the call (`{A}` the function), its parameters, what
/// it answers (`{D}` the item's `did ${x}`), and the leaf it lowers to. The ones that look for an
/// item stop at the second, as JavaScript's do. A reviver and a replacer pass the whole and the
/// list on as they are.
const METHODS: &[(&str, &str, &str, &str, &str)] = &[
    ("map", "xs.map({A})", "x", "{D}", "map"),
    ("flatMap", "xs.flatMap({A})", "x", "[{D}]", "map"),
    ("Array.from", "Array.from(xs, {A})", "x", "{D}", "map"),
    ("filter", "xs.filter({A})", "x", "{D} !== \"did zz2\"", "filter"),
    ("some", "xs.some({A})", "x", "{D} === \"did zz2\"", "any"),
    ("every", "xs.every({A})", "x", "{D} !== \"did zz2\"", "all"),
    ("find", "xs.find({A})", "x", "{D} === \"did zz2\"", "find"),
    ("findIndex", "xs.findIndex({A})", "x", "{D} === \"did zz2\"", "find_index"),
    ("findLast", "xs.findLast({A})", "x", "{D} === \"did zz2\"", "find_last"),
    ("findLastIndex", "xs.findLastIndex({A})", "x", "{D} === \"did zz2\"", "find_last_index"),
    ("reduce", "xs.reduce({A}, \"\")", "acc, x", "acc + {D}", "fold"),
    ("reduceRight", "xs.reduceRight({A}, \"\")", "acc, x", "acc + {D}", "fold"),
    ("toSorted", "xs.toSorted({A})", "x, y", "{D} < `did ${y}` ? -1 : 1", "sort_by"),
    ("Object.groupBy", "Object.groupBy(xs, {A})", "x", "{D}", "group_by"),
    ("forEach", "xs.forEach({A})", "x", "{D}", ""),
    ("JSON.parse", "JSON.parse(JSON.stringify(xs), {A})", "k, x", "Array.isArray(x) ? x : {D}", "json_parse"),
    ("JSON.stringify", "JSON.stringify(xs, {A})", "k, x", "typeof x === \"string\" && x.startsWith(\"zz\") ? {D} : x", "json_stringify"),
];

/// Where the effect is written: in a function the method's function calls; in the flow's function
/// given by its name; in the arrow's own expression; in a branch of the arrow taken for the second
/// item alone; or in a function that makes it and then fails on one item (a `JSON.parse` of what
/// isn't JSON), where JavaScript stops at the item that throws.
const FORMS: &[&str] = &["a function", "by name", "in the arrow", "in a branch, ?:", "in a branch, ??", "in a branch, &&", "in a branch, ||", "fails at zz1", "fails at zz2", "fails at zz3"];

/// The ways a list call is reached: a name, and a handler's body with `{M}` the call. In a
/// function (`{F}`, written after `@FUNCTION@` and named for its handler), the list is the
/// function's. Section E's rows close it: a list in a list's function, in a catch, in a finally,
/// and on the sides of `&&`, `||` and `??`, read and not.
const SHAPES: &[(&str, &str)] = &[
    ("plain", "{X}\nconst got = {M}\nreturn got"),
    ("in a try that then fails", "{X}\ntry {\nconst got = {M}\nconst parsed = JSON.parse(req.json.bad)\nreturn got\n} catch {\nreturn \"bad\"\n}"),
    ("in a try", "{X}\ntry {\nconst got = {M}\nreturn got\n} catch {\nreturn \"bad\"\n}"),
    ("past a return that answered", "{X}\nif (req.json.a === \"zz1\") {\nreturn \"early\"\n}\nconst got = {M}\nreturn got"),
    ("in a branch not taken", "{X}\nconst got = req.json.a === \"zz9\" ? {M} : \"no\"\nreturn got"),
    ("in an if not entered", "{X}\nif (req.json.a === \"zz9\") {\nconst got = {M}\nreturn got\n}\nreturn \"no\""),
    ("in a function not entered", "{X}\nconst got = req.json.a === \"zz9\" ? {F}(xs) : \"no\"\nreturn got\n@FUNCTION@\nfunction {F}(xs) {\n  const got = {M}\n  return got\n}\n"),
    ("of constants", "const xs = [\"zz1\", \"zz2\", \"zz3\"]\nconst got = {M}\nreturn got"),
    (
        "in a try in an arm taken",
        "{X}\nconst got = req.json.a !== \"zz9\" ? {F}(xs) : \"no\"\nreturn got\n@FUNCTION@\nfunction {F}(xs) {\n  try {\n    const got = {M}\n    return got\n  } catch {\n    return \"bad\"\n  }\n}\n",
    ),
    (
        "in a try in an arm not taken",
        "{X}\nconst got = req.json.a === \"zz9\" ? {F}(xs) : \"no\"\nreturn got\n@FUNCTION@\nfunction {F}(xs) {\n  try {\n    const got = {M}\n    return got\n  } catch {\n    return \"bad\"\n  }\n}\n",
    ),
    ("in a list's function", "{X}\nconst got = [0].map((y) => {M})\nreturn got"),
    ("in a catch", "{X}\ntry {\nconst parsed = JSON.parse(req.json.bad)\nreturn \"parsed\"\n} catch {\nconst got = {M}\nreturn got\n}"),
    ("in a finally", "{X}\ntry {\nconst parsed = JSON.parse(req.json.bad)\n} catch {\nreturn \"bad\"\n} finally {\nconst got = {M}\n}\nreturn \"end\""),
    ("after && taken", "{X}\nconst got = req.json.a === \"zz1\" && {M}\nreturn got"),
    ("after && not taken", "{X}\nconst got = req.json.a === \"zz9\" && {M}\nreturn got"),
    ("after || not taken", "{X}\nconst got = req.json.a === \"zz1\" || {M}\nreturn got"),
    ("after ?? taken", "{X}\nconst got = req.json.none ?? {M}\nreturn got"),
    ("after ?? not taken", "{X}\nconst got = req.json.a ?? {M}\nreturn got"),
];

/// The list of the request's items, as each shape begins.
const XS: &str = "const xs = [req.json.a, req.json.b, req.json.c]";

/// The shapes a function that fails on an item is reached in.
const FAILING_SHAPES: &[&str] = &["plain", "in a try", "in a try in an arm taken", "in a try in an arm not taken"];

/// One handler of the list grid.
pub(super) struct ListCase {
    shape: &'static str,
    method: &'static str,
    leaf: &'static str,
    form: &'static str,
    kind: &'static str,
    /// The handler's body, and after `@FUNCTION@` a function of the flow's it calls, `{F}`.
    body: String,
}

impl ListCase {
    /// The handler's body and the function it calls, if any, that function named `name`.
    fn parts(&self, name: &str) -> (String, String) {
        let (body, function) = self.body.split_once("\n@FUNCTION@\n").unwrap_or((&self.body, ""));
        (body.replace("{F}", name), function.replace("{F}", name))
    }
}

/// The method's function, in `form`.
fn arrow(form: &str, params: &str, answers: &str, kind: &str, statement: &str) -> String {
    let answer = |did: &str| answers.replace("{D}", did);
    match form {
        "a function" => format!("({params}) => {}", answer(&format!("k{kind}(x)"))),
        "by name" if params.starts_with("k,") => format!("k{kind}j"),
        "by name" if params.contains(',') => format!("k{kind}p"),
        "by name" => format!("k{kind}"),
        "in a branch, ?:" => format!("({params}) => {}", answer(&format!("(x === \"zz2\" ? k{kind}(x) : `did ${{x}}`)"))),
        "in a branch, ??" => format!("({params}) => {}", answer(&format!("(({{ zz1: \"did zz1\", zz3: \"did zz3\" }})[x] ?? k{kind}(x))"))),
        // `&&` and `||` take Booleans in a flow, so each is a test, and the value the item's text.
        "in a branch, &&" => format!("({params}) => {}", answer(&format!("(x === \"zz2\" && k{kind}(x) === \"\" ? \"\" : `did ${{x}}`)"))),
        "in a branch, ||" => format!("({params}) => {}", answer(&format!("(x !== \"zz2\" || k{kind}(x) !== \"\" ? `did ${{x}}` : \"\")"))),
        f if f.starts_with("fails at zz") => format!("({params}) => {}", answer(&format!("k{kind}f{}(x)", &f["fails at zz".len()..]))),
        _ => format!("({params}) => {}", answer(&format!("({statement})"))),
    }
}

pub(super) fn list_cases() -> Vec<ListCase> {
    let mut all = Vec::new();
    for (shape, text) in SHAPES {
        for (method, call, params, answers, leaf) in METHODS {
            for form in FORMS {
                if form.starts_with("fails at") && !FAILING_SHAPES.contains(shape) {
                    continue;
                }
                for (kind, statement) in EFFECTS {
                    let body = text.replace("{X}", XS).replace("{M}", &call.replace("{A}", &arrow(form, params, answers, kind, statement)));
                    all.push(ListCase { shape, method, leaf, form, kind, body });
                }
            }
        }
    }
    all
}

/// The functions `handlers` call, as the flow and JavaScript both have them.
fn functions(handlers: &[&ListCase]) -> String {
    let uses = |what: &str| handlers.iter().any(|c| c.body.contains(what));
    let mut text = String::new();
    for (kind, statement) in EFFECTS {
        let waits = if statement.contains("await ") { "async " } else { "" };
        let json = uses(&format!("k{kind}j"));
        if json || uses(&format!("k{kind})")) || uses(&format!("k{kind}(")) {
            text.push_str(&format!("\n{waits}function k{kind}(x) {{\n  {statement}\n  return `did ${{x}}`\n}}\n"));
        }
        if uses(&format!("k{kind}p")) {
            text.push_str(&format!("\n{waits}function k{kind}p(a, x) {{\n  {statement}\n  return `${{a}}did ${{x}}`\n}}\n"));
        }
        if json {
            text.push_str(&format!("\nfunction k{kind}j(k, x) {{\n  return Array.isArray(x) || k === \"\" ? x : k{kind}(x)\n}}\n"));
        }
        for n in 1..=3 {
            if uses(&format!("k{kind}f{n}(")) {
                text.push_str(&format!("\n{waits}function k{kind}f{n}(x) {{\n  {statement}\n  const parsed = JSON.parse(x === \"zz{n}\" ? \"{{\" : \"\\\"\\\"\")\n  return `did ${{x}}${{parsed}}`\n}}\n"));
            }
        }
    }
    text
}

/// A flow of `handlers`, each a route `POST /l<i>`, calling out to `host`.
fn flow_text(name: &str, port: u16, host: &str, handlers: &[(usize, &ListCase)]) -> String {
    let defined = functions(&handlers.iter().map(|(_, c)| *c).collect::<Vec<_>>());
    let uses = |what: &str| defined.contains(what) || handlers.iter().any(|(_, c)| c.body.contains(what));
    let mut text = format!("import {{ {}flow, route }} from \"holon\"\nimport state from \"state\"\n", if uses("ping(") { "action, " } else { "" });
    for (used, import) in [("marks.", "collection:marks"), ("news.", "channel:news"), ("jobs.", "queue:jobs"), ("ops.", "mail:ops")] {
        if uses(used) {
            text.push_str(&format!("import {} from \"{import}\"\n", used.trim_end_matches('.')));
        }
    }
    if uses("api.") {
        text.push_str(&format!("import api from \"host:{host}\"\n"));
    }
    text.push_str(&format!("\nflow(\"{name}\", {{ port: {port} }})\n{defined}"));
    if uses("ping(") {
        text.push_str("\nexport const ping = action(() => ++state.pinged)\n");
    }
    for (i, case) in handlers {
        let (body, function) = case.parts(&format!("each{i}"));
        if !function.is_empty() {
            text.push_str(&format!("\n{function}"));
        }
        text.push_str(&format!("\nroute.post(\"/l{i}\", (req) => {{\n{body}\n}})\n"));
    }
    text
}

/// What was made, kind by kind in the order made: each kind's entries, `c:zz1 c:zz2`, the kinds
/// in the order of their letters.
fn by_kind(entries: &[String]) -> String {
    let mut sorted = entries.to_vec();
    // A stable sort by the kind keeps each kind's order.
    sorted.sort_by(|a, b| a.split(':').next().cmp(&b.split(':').next()));
    sorted.join(" ")
}

/// What JavaScript made of a list handler, kind by kind (`by_kind`), whether it threw, and its
/// answer.
#[derive(Debug)]
struct JsDid {
    made: String,
    threw: bool,
    answer: String,
}

/// What JavaScript does with each of `handlers` on `LIST_REQUEST`, with the try's promise written
/// as JavaScript (`mark`, `undo`, which keeps the requests asked). Its stand-in for a host answers
/// at once, so a function that waits on one is written without the wait.
fn javascript_does(handlers: &[&ListCase]) -> Vec<JsDid> {
    let made: Vec<String> = handlers
        .iter()
        .map(|c| {
            let (body, function) = c.parts("each");
            let undone = |t: &str| t.replace("try {\n", "const TM = mark()\ntry {\n").replace("} catch {\n", "} catch {\nundo(TM)\n");
            let text = format!("{}\n{}", functions(&[c]), undone(&function)).replace("async function", "function").replace("await ", "");
            format!("(env) => {{\nconst {{ state, marks, news, jobs, ops, console, ping, api, mark, undo }} = env;\n{text}\nreturn (req) => {{\n{}\n}}\n}}", undone(&body))
        })
        .collect();
    let source = r#"(req) => {
  // Object.groupBy as the specification has it, where the engine lacks it.
  if (!Object.groupBy) Object.groupBy = (xs, f) => {
    const out = Object.create(null);
    let i = 0;
    for (const x of xs) { const k = f(x, i++); (out[k] ??= []).push(x) }
    return out;
  };
  const handlers = [
@HANDLERS@
  ];
  return handlers.map((make) => {
    const fx = [];
    const st = {};
    const env = {
      state: new Proxy(st, { set(t, k, v) { fx.push(k + ":" + v); t[k] = v; return true } }),
      marks: { put: (k, v) => { fx.push("p:" + v) } },
      news: { send: (m) => { fx.push("c:" + m.n) } },
      jobs: { send: (m) => { fx.push("q:" + m.data.n) } },
      ops: { send: (m) => { fx.push("m:" + m.subject) } },
      console: { log: (x) => { fx.push("l:" + x) } },
      ping: (a) => { fx.push("s:" + a.n); return { run: "r" } },
      api: { post: (path, o) => { fx.push("r:" + o.body); return { status: 200 } } },
      mark: () => [fx.length, Object.assign({}, st)],
      undo: ([n, kept]) => {
        // A request asked is asked: the try's promise holds back what it made, not what it asked.
        fx.push(...fx.splice(n).filter((e) => e.startsWith("r:")));
        for (const k of Object.keys(st)) delete st[k];
        Object.assign(st, kept);
      },
    };
    let answer, threw = false;
    try { answer = make(env)(req) } catch (e) { answer = "threw " + e; threw = true }
    return { made: fx, threw, answer: typeof answer === "string" ? answer : (JSON.stringify(answer) ?? "") };
  });
}"#
    .replace("@HANDLERS@", &made.join(",\n"));
    let mut request = BTreeMap::from([("body".to_string(), Value::Text(LIST_REQUEST.to_string()))]);
    request.insert("json".to_string(), holon_core::parse_json(LIST_REQUEST).unwrap());
    let limits = holon_core::ModuleLimits { fuel: 4_000_000_000, memory: 256 << 20 };
    let outs = holon_module::shared().run(holon_module::JS_ENGINE, &limits, &[Some(Value::Text(source)), Some(Value::List(vec![Value::Record(request)]))]).unwrap();
    let Some(Value::Record(answer)) = outs.into_iter().next().flatten() else { panic!("the engine answers a record") };
    let Some(Value::List(per_handler)) = answer.get("value") else { panic!("JavaScript failed: {answer:?}") };
    per_handler
        .iter()
        .map(|v| {
            let Value::Record(r) = v else { panic!("{v:?}") };
            let (Some(Value::List(made)), Some(Value::Boolean(threw)), Some(Value::Text(answer))) = (r.get("made"), r.get("threw"), r.get("answer")) else { panic!("{r:?}") };
            let made: Vec<String> = made.iter().map(|e| if let Value::Text(t) = e { t.clone() } else { panic!("{e:?}") }).collect();
            JsDid { made: by_kind(&made), threw: *threw, answer: answer.clone() }
        })
        .collect()
}

/// What the run of `POST /l<i>` made, kind by kind (`by_kind`), each entry the item it carries,
/// read from what the log gained; and its answer.
fn flow_does(state: &AppState, slug: &str, i: usize, seen: &mut Option<u64>) -> (String, String) {
    let (_, answer, _) = data_call(state, slug, "POST", &format!("/l{i}"), LIST_REQUEST);
    let after = seen.map(|seq| format!("after={seq}&")).unwrap_or_default();
    let gained = body_json(route(state, &req("GET", &format!("/deployments/{slug}/log?{after}limit=50"), vec![])));
    let item = |v: &serde_json::Value| {
        let s = v.to_string();
        ITEMS.iter().find(|x| s.contains(*x)).copied().unwrap_or("?")
    };
    let mut made: Vec<String> = Vec::new();
    for e in gained["entries"].as_array().unwrap() {
        *seen = (*seen).max(e["seq"].as_u64());
        if e["route"] == "action ping" {
            made.push(format!("s:{}", item(&e["request"])));
            continue;
        }
        if e["kind"] != "request" {
            continue;
        }
        for round in e["rounds"].as_array().into_iter().flatten() {
            for exchange in round["exchanges"].as_array().into_iter().flatten().filter(|x| x["request"].to_string().contains("/in")) {
                made.push(format!("r:{}", item(&exchange["request"])));
            }
        }
        for p in e["performed"].as_array().into_iter().flatten() {
            let mark = match p["capability"].as_str().unwrap_or("") {
                "StateWrite(\"w\")" => "w",
                "CollectionWrite(\"marks\")" => "p",
                "ChannelSend(\"news\")" => "c",
                "QueueSend(\"jobs\")" => "q",
                "MailSend(\"ops\")" => "m",
                other => panic!("an effect the list grid doesn't know: {other}"),
            };
            made.push(format!("{mark}:{}", item(&p["payload"])));
        }
        let lines = body_json(route(state, &req("GET", &format!("/deployments/{slug}/logs?run={}", e["seq"]), vec![])));
        for line in lines["lines"].as_array().into_iter().flatten() {
            made.push(format!("l:{}", item(line)));
        }
    }
    (by_kind(&made), answer)
}

/// The debts a list handler that differs is owed under, each matched by the one difference it
/// names; any other difference fails.
const LIST_DEBTS: &[&Debt] = &[
    &A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE,
    &THE_STARTED_HALF,
    &A_LISTS_FUNCTION_GOES_PAST_A_FAILURE,
    &A_REVIVERS_NOTHING_IS_NULL,
    &A_FOLD_STOPS_AT_NOTHING,
];

/// Each kind's items, in the order made, from `by_kind`'s `c:zz1 c:zz2`.
fn per_kind(made: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut out: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for e in made.split(' ').filter(|e| !e.is_empty()) {
        let (kind, item) = e.split_once(':').unwrap_or((e, ""));
        out.entry(kind).or_default().push(item);
    }
    out
}

/// What a comparator's calls make is left to the engine: JavaScript doesn't say how often or in
/// what order `toSorted` calls its function, so what it makes there, of any kind, is not compared.
/// The text without it.
fn without_a_comparators_calls(case: &ListCase, made: &str) -> String {
    if case.method != "toSorted" {
        return made.to_string();
    }
    String::new()
}

/// The debts (`LIST_DEBTS`) a differing list handler is owed under, kind by kind: each kind that
/// differs must be explained by one, or the handler is wrong (None).
fn list_debt(case: &ListCase, made: &str, answer: &str, js: &JsDid) -> Option<Vec<&'static Debt>> {
    let (ours, theirs) = (per_kind(made), per_kind(&js.made));
    let kinds: BTreeSet<&str> = ours.keys().chain(theirs.keys()).copied().collect();
    let fails = case.form.starts_with("fails at");
    // A try block that fails: the shape's, or the shape's try around a function that fails.
    let failing_try = case.shape == "in a try that then fails" || (fails && case.shape.starts_with("in a try") && !case.shape.ends_with("not taken"));
    let mut owed: Vec<&'static Debt> = Vec::new();
    let mut owe = |d: &'static Debt| {
        if !owed.iter().any(|o| o.name == d.name) {
            owed.push(d);
        }
    };
    let none = Vec::new();
    for kind in kinds {
        let (o, t) = (ours.get(kind).unwrap_or(&none), theirs.get(kind).unwrap_or(&none));
        if o == t {
            continue;
        }
        // The flow went on from where JavaScript stopped: JavaScript's items, then more.
        let goes_on = o.len() > t.len() && o.starts_with(t);
        // S: a try block that fails keeps what a function in it made, a list's as any.
        if "wpcqml".contains(kind) && failing_try && t.is_empty() {
            owe(&A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE);
        } else if kind == "s" && failing_try && t.is_empty() {
            owe(&THE_STARTED_HALF);
        } else if fails && goes_on && case.method.starts_with("JSON.") && answer.contains("null") {
            owe(&A_REVIVERS_NOTHING_IS_NULL);
        } else if fails && goes_on {
            owe(&A_LISTS_FUNCTION_GOES_PAST_A_FAILURE);
        } else if case.method.starts_with("reduce") && case.form == "in the arrow" && kind == "s" && o.len() == 1 && t.starts_with(o) {
            owe(&A_FOLD_STOPS_AT_NOTHING);
        } else {
            return None;
        }
    }
    (!owed.is_empty()).then_some(owed)
}

/// Handlers the reader takes and `lang::compile` refuses: each debt, and the cases owed under it.
type Owes = fn(&ListCase) -> bool;
const ROUND_TRIP_DEBTS: &[(&str, Owes)] = &[
    // Struck by C3 (2026-11-24): "the printer can't write a list's function that uses values from
    // around it" (a list in a list's function, the inner arrow using `xs`), written back since.
];

/// **Guards:** a list method's function makes what the functions it calls make, once per item it
/// reaches and in order, as JavaScript does; and nothing where the run doesn't go. Where the flow
/// differs, the difference is owed under a named debt of reached (`LIST_DEBTS`, each with its
/// smallest flow in `reached.rs`); any other difference fails, and so does a debt owed nowhere.
///
/// **Why:** a list method's function compiles to a graph run once per item, and what a statement
/// does or not is which of its nodes' triggers is tied to what. A start or a write placed there
/// can go out where the text says the run never goes, or be dropped where it says it's made.
///
/// **If it fails:** a row names the shape, the method, where the effect is written and its kind,
/// what the flow made and what JavaScript makes. A graph that can't run (a cycle, an `after` with
/// nothing to tie) fails too; any other refusal is said at check, and is printed, not failed.
#[test]
#[ignore = "heavy: about 5 minutes, run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_list_methods_function_makes_what_it_says_item_by_item() {
    let sink = smtp::sink::start("PLAIN LOGIN");
    let sqs = sqs::stand_in::StandIn::start();
    sqs.make("jobs");
    let (state, secrets) = super::semantics::grid_state(&sink, &sqs);
    let (host, _hits) = counting_server();
    let all = list_cases();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let mut wrong: Vec<String> = Vec::new();
    // The class, from core's own list: every leaf that takes a function is reached by a row.
    for &leaf in holon_core::PASSES_EXTRA {
        if !all.iter().any(|c| c.leaf == leaf) {
            wrong.push(format!("{leaf} takes a function (PASSES_EXTRA) and no row reaches it: add the method that lowers to it to METHODS"));
        }
    }
    let indexed: Vec<(usize, &ListCase)> = all.iter().enumerate().collect();
    let checked = thread::scope(|s| {
        let checking: Vec<_> = indexed
            .chunks(indexed.len().div_ceil(workers))
            .map(|part| s.spawn(|| part.iter().map(|&(i, c)| ((i, c), holon_author::agent::plan(&flow_text("probe", 23000, &host, &[(i, c)])).map(|_| ()))).collect::<Vec<_>>()))
            .collect();
        checking.into_iter().flat_map(|c| c.join().unwrap()).collect::<Vec<_>>()
    });
    // What the language refuses, by shape, form and reason: the methods and kinds. A graph that
    // can't run is the compiler's own fault, and fails.
    type Which<'a> = (BTreeSet<&'a str>, BTreeSet<&'a str>);
    let mut refused: BTreeMap<(&str, &str, String), Which> = BTreeMap::new();
    let mut round_trip_owed: BTreeMap<&str, usize> = BTreeMap::new();
    let mut read_alone: Vec<(usize, &ListCase)> = Vec::new();
    for ((i, c), r) in &checked {
        let Err(e) = r else { continue };
        let text = flow_text("probe", 23000, &host, &[(*i, c)]);
        if holon_author::lang::ts::compile(&text).is_ok() && holon_author::lang::compile(&text).is_err() {
            read_alone.push((*i, c));
            match ROUND_TRIP_DEBTS.iter().find(|(_, owes)| owes(c)) {
                Some((debt, _)) => *round_trip_owed.entry(debt).or_default() += 1,
                None => wrong.push(format!("{} | {} | {} | {} | the canvas can't write it back: {}", c.shape, c.method, c.form, c.kind, e.lines().next().unwrap_or(""))),
            }
            continue;
        }
        if e.contains("cycle") || e.contains("this statement makes none") {
            wrong.push(format!("{} | {} | {} | {} | a graph that can't run: {e}", c.shape, c.method, c.form, c.kind));
        }
        let first = e.lines().next().unwrap_or("");
        let why = first.strip_prefix("text:").and_then(|r| r.splitn(3, ':').nth(2)).map_or(first, str::trim_start).to_string();
        let at = refused.entry((c.shape, c.form, why)).or_default();
        at.0.insert(c.method);
        at.1.insert(c.kind);
    }
    for ((shape, form, why), (methods, kinds)) in &refused {
        println!("  refused: {shape} | {form} | {} | {} | {why}", methods.iter().copied().collect::<Vec<_>>().join(" "), kinds.iter().copied().collect::<Vec<_>>().join(""));
    }
    let taken: Vec<(usize, &ListCase)> = checked.iter().filter(|(_, r)| r.is_ok()).map(|(c, _)| *c).collect();
    /// A flow of the grid: its number, its handlers, and whether it is deployed read alone.
    type ListFlow<'a> = (usize, &'a [(usize, &'a ListCase)], bool);
    let flows: Vec<ListFlow> = taken.chunks(40).map(|c| (c, false)).chain(read_alone.chunks(40).map(|c| (c, true))).enumerate().map(|(n, (c, alone))| (n, c, alone)).collect();
    let rows: Vec<(&ListCase, bool, String, String, JsDid)> = thread::scope(|s| {
        let running: Vec<_> = flows
            .chunks(flows.len().div_ceil(workers).max(1))
            .map(|part| {
                let (state, host) = (&state, &host);
                s.spawn(move || {
                    let mut rows = Vec::new();
                    for (n, chunk, alone) in part {
                        let text = flow_text(&format!("list-{n}"), 23001 + *n as u16, host, chunk);
                        let slug = if *alone {
                            deploy_read_alone(state, &text, host)
                        } else if text.contains("host:") {
                            deploy_text(state, &text, host)
                        } else {
                            deploy_plain_text(state, &text)
                        };
                        let javascript = javascript_does(&chunk.iter().map(|(_, c)| *c).collect::<Vec<_>>());
                        let mut seen = None;
                        for ((i, case), js) in chunk.iter().zip(javascript) {
                            let (made, answer) = flow_does(state, &slug, *i, &mut seen);
                            rows.push((*case, *alone, made, answer, js));
                        }
                    }
                    rows
                })
            })
            .collect();
        running.into_iter().flat_map(|r| r.join().unwrap()).collect()
    });
    let (mut alike, mut owed_runs, mut comparators) = (0, 0, 0);
    let mut owed = BTreeMap::<&str, usize>::new();
    for (case, alone, made, answer, js) in &rows {
        if *made == js.made {
            alike += 1;
            continue;
        }
        let (made, javascript) = (without_a_comparators_calls(case, made), without_a_comparators_calls(case, &js.made));
        if made == javascript {
            comparators += 1;
            continue;
        }
        let js = &JsDid { made: javascript, threw: js.threw, answer: js.answer.clone() };
        match list_debt(case, &made, answer, js) {
            Some(owing) => {
                owed_runs += 1;
                owing.iter().for_each(|d| *owed.entry(d.name).or_default() += 1);
            }
            None => wrong.push(format!(
                "{}{} | {} | {} | {} | the flow made [{made}] answering {answer} | JavaScript makes [{}] answering {}",
                if *alone { "[read alone] " } else { "" },
                case.shape,
                case.method,
                case.form,
                case.kind,
                js.made,
                js.answer
            )),
        }
    }
    // Each run under one heading; the wrong are runs, and handlers the canvas can't write back
    // under no debt.
    println!(
        "{} list handlers, {} taken, {} read alone, {} runs: {alike} made as JavaScript makes them, {comparators} alike but for a comparator's calls (left to the engine), {owed_runs} owed, {} wrong",
        all.len(),
        taken.len(),
        read_alone.len(),
        rows.len(),
        wrong.len()
    );
    for (debt, n) in &round_trip_owed {
        println!("  {n:4} list handlers the canvas can't write back, under \"{debt}\"");
    }
    // A run owed under two debts shows under each.
    for (owing, n) in &owed {
        println!("  {n:4} runs owed under \"{owing}\"");
    }
    let _ = std::fs::remove_dir_all(&secrets);
    assert!(wrong.is_empty(), "{} list handlers don't make what their text says:\n{}", wrong.len(), wrong.join("\n"));
    for (debt, _) in ROUND_TRIP_DEBTS {
        assert!(round_trip_owed.contains_key(debt), "\"{debt}\": the canvas writes these back now: strike it from ROUND_TRIP_DEBTS");
    }
    for d in LIST_DEBTS {
        assert!(owed.contains_key(d.name), "\"{}\" no longer differs anywhere: strike it from LIST_DEBTS ({})", d.name, d.says);
    }
}
