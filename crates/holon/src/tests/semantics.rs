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
//! either (docs/LANGUAGE.md §5.6), and an `if` on a condition that isn't there runs neither branch,
//! nor what follows it if a branch returns (§6.4). That rule is written here once, as JavaScript
//! (`AS_A_FLOW_READS`, and `neither` for an `if`), and every condition has the form it takes under
//! it. So each handler is run three ways: as the flow, as JavaScript, and as JavaScript with that
//! rule. The flow does what the third does, always; where the third differs from the second
//! is the whole of what a reader of JavaScript has to know, and is counted.
//!
//! It keeps one promise JavaScript doesn't, also on purpose: a `try` block that fails leaves
//! nothing behind but the requests it made (docs/LANGUAGE.md §6.5), written here as JavaScript
//! once (`mark` as the block begins, `undo` in its catch). Outside a try, a throw ends the handler
//! as a return of nothing, and so, as in JavaScript, does a line that fails (the owner's open
//! question: today a flow goes on, owed under its debt).
//!
//! Where a flow doesn't yet do what it reads as, the difference is owed under a named debt of
//! reached (`reached.rs`, each with its smallest flow), matched mark by mark (`debt`): any other
//! difference fails, and so does a debt owed nowhere.
//!
//! To add a case: a new way to reach a statement is a line of `SHAPES`, a new kind of effect a line
//! of `EFFECTS` (and its mark where the log is read, `flow_does`), a new condition a line of
//! `CONDITIONS`, a new request a line of `REQUESTS`. None needs its answer written.

use super::reached::{
    Debt, AN_ARMS_CALL_READ_ONLY_ELSEWHERE_IS_NOT_MADE, AN_UNUSED_READ_IS_NOT_ASKED, A_CHOICES_TEST_IS_READ_PAST_A_RETURN, A_READ_IS_ASKED_PAST_A_RETURN, A_CALLED_START_READ_ONLY_ELSEWHERE_IS_NOT_MADE, A_CHOICE_IN_A_FUNCTION_STARTS, A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE, A_FINALLY_FORGETS_THE_RETURN_ABOVE,
    A_FUNCTIONS_OWN_CONDITION, A_LINE_AFTER_A_FAILURE_RUNS, A_LISTS_FUNCTION_LOSES_ITS_WRITE, A_LISTS_FUNCTION_STARTS_PAST_A_RETURN,
};
use super::*;

/// The kinds of thing a handler does: the mark each leaves, the statement that does it, and the
/// same as an expression whose value is used. `rec` is a function that writes, `ask` one that
/// calls out, `ping` an action of the flow's. A function places functions, so each is also reached
/// through others that return none of what it does (`rec2`, `rec3`, `ask2`) or only a part of it
/// (`part`, which returns `both`'s put and not its write): what a function does, the one placing
/// it does. `news` is a channel, `gone` a collection a key is deleted from. `bump` writes and is
/// bound to a name; `go` starts `ping`, `maybe` starts it in an arm of `?:` of its own; `askif`
/// calls out under a condition of its own; `held` has a try block of its own that writes and puts
/// and then reads its text as JSON, and sends in its catch (given what isn't JSON, `ft`, its block
/// fails; given JSON, `fu`, it doesn't); `fin` returns early when given "a", and else enters a try
/// whose finally writes. A list's `.map` places a function that writes (`lf`) or starts (`ls`).
const EFFECTS: &[(&str, &str, &str)] = &[
    ("w", "++state.w", ""),
    ("ws", "state.w = \"v\"", ""),
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
    ("c", "news.send({ n: 1 })", ""),
    ("d", "delete gone[\"k\"]", ""),
    ("fb", "const n = bump(\"a\")", "bump(\"a\")"),
    ("fs", "go(\"a\")", "go(\"a\")"),
    ("fr", "maybe(\"a\")", "maybe(\"a\")"),
    ("gi", "const asked = await askif(\"a\")", "await askif(\"a\")"),
    ("ft", "held(\"a\")", "held(\"a\")"),
    ("fu", "held(\"{}\")", "held(\"{}\")"),
    ("fy", "fin(req.json.k ?? \"none\")", "fin(req.json.k ?? \"none\")"),
    ("lf", "const each = [req.body].map((x) => rec(x))", "[req.body].map((x) => rec(x))"),
    ("ls", "const each = [req.body].map((x) => ping(x))", "[req.body].map((x) => ping(x))"),
    ("q", "jobs.send({ data: { n: 1 } })", ""),
    ("m", "ops.send({ to: \"a@acme.com\", subject: \"s\", text: \"hi\" })", ""),
    ("fa", "every(\"a\")", "every(\"a\")"),
    ("fa2", "every2(\"a\")", "every2(\"a\")"),
    ("rd", "const got = notes[\"k\"]", "notes[\"k\"]"),
    ("pk", "marks[notes[req.json.k] ?? \"k\"] = \"v\"", ""),
    // A folder's file written (`files`, bound read-write), a database's row (`shop`), a file sent
    // by handle (an upload), a call streamed, and another flow's action called (`callee`'s `hail`).
    ("h", "files[\"k.txt\"] = \"v\"", ""),
    ("b", "shop.exec(\"insert into t (v) values ($1)\", \"v\")", ""),
    ("u", "const up = await api.put(\"/up\", { file: files.file(\"a.txt\") })", "await api.put(\"/up\", { file: files.file(\"a.txt\") })"),
    ("t", "const st = await api.post(\"/st\", { body: \"x\", stream: true })", "await api.post(\"/st\", { body: \"x\", stream: true })"),
    ("k", "const called = await hail({})", "await hail({})"),
    // A throw: its message goes to the run's log (docs/LANGUAGE.md §6.5a), in a try block too,
    // which JavaScript keeps nowhere.
    ("x", "throw new Error(\"boom\")", ""),
];

/// The forms a condition takes, over fields a request may lack: as written, as a flow reads it
/// (`AS_A_FLOW_READS`), and as a flow reads it where it is a value and not a condition (the left
/// of `&&`), where `&&` and `||` take Booleans and nothing else.
pub(super) const CONDITIONS: &[(&str, &str, &str)] = &[
    ("req.json.c", "J.c", "J.c"),
    ("!req.json.c", "N(J.c)", "N(J.c)"),
    ("req.json.n > 10", "GT(J.n, 10)", "GT(J.n, 10)"),
    ("req.json.n <= 10", "LE(J.n, 10)", "LE(J.n, 10)"),
    ("req.json.s === \"x\"", "EQ(J.s, \"x\")", "EQ(J.s, \"x\")"),
    ("req.json.s !== \"x\"", "NE(J.s, \"x\")", "NE(J.s, \"x\")"),
    ("req.json.c && req.json.d", "AND(J.c, J.d)", "ANDV(J.c, J.d)"),
    ("req.json.c || req.json.d", "OR(J.c, J.d)", "ORV(J.c, J.d)"),
    ("!(req.json.n > 10)", "N(GT(J.n, 10))", "N(GT(J.n, 10))"),
    // A read in the test (the collection answers nothing for every key), and a flow function's
    // call that only computes.
    ("notes[req.json.k] === \"v\"", "EQ(notes[J.k], \"v\")", "EQ(notes[J.k], \"v\")"),
    ("check(req.json.k)", "EQ(J.k, \"a\")", "EQ(J.k, \"a\")"),
];

/// How a flow reads what JavaScript computes with `undefined`, as JavaScript: nothing in, nothing
/// out. `!` of nothing is nothing; a comparison takes two numbers or two texts, and anything else
/// gives nothing; equality of nothing is nothing. `&&` and `||` read left to right and stop at
/// the side that decides, as JavaScript's do (`false && nothing` is false, `true || nothing` true),
/// but a side they reach that is nothing makes them nothing (`nothing || true` is nothing, where
/// JavaScript's is true). Where they are a value and not a condition (`ANDV`, `ORV`; `ANDL` and
/// `ORL` with the right side run only when reached), they take Booleans: anything else is nothing.
/// In a condition, a side is read as JavaScript reads it (`ANDC`, `ORC`, the right side run only
/// when reached).
pub(super) const AS_A_FLOW_READS: &str = r#"
    const N = (x) => x === undefined ? undefined : !x;
    const alike = (a, b) => typeof a === typeof b && (typeof a === "number" || typeof a === "string");
    const GT = (a, b) => alike(a, b) ? a > b : undefined;
    const LE = (a, b) => alike(a, b) ? a <= b : undefined;
    const EQ = (a, b) => a === undefined || b === undefined ? undefined : a === b;
    const NE = (a, b) => a === undefined || b === undefined ? undefined : a !== b;
    const AND = (a, b) => a === undefined ? undefined : !a ? false : b === undefined ? undefined : !!b;
    const OR = (a, b) => a === undefined ? undefined : !!a ? true : b === undefined ? undefined : !!b;
    const B = (x) => typeof x === "boolean" ? x : undefined;
    const ANDL = (a, f) => B(a) === undefined ? undefined : !a ? false : B(f());
    const ORL = (a, f) => B(a) === undefined ? undefined : a ? true : B(f());
    const ANDV = (a, b) => ANDL(a, () => b);
    const ORV = (a, b) => ORL(a, () => b);
    const T = (x) => x === undefined ? undefined : !!x;
    const ANDC = (a, f) => a === undefined ? undefined : !a ? false : T(f());
    const ORC = (a, f) => a === undefined ? undefined : a ? true : T(f());
    const PARSED = (t) => { try { return JSON.parse(t) } catch (e) { return undefined } };
"#;

/// Requests' bodies: fields that hold, that don't, that are missing, of another type, and a body
/// that isn't JSON at all.
pub(super) const REQUESTS: &[&str] = &[
    r#"{"c":true,"d":true,"n":50,"s":"x","k":"a","v":"kept"}"#,
    r#"{"c":false,"d":false,"n":5,"s":"y","k":"b"}"#,
    r#"{}"#,
    r#"{"c":"yes","d":0,"n":"50","s":1,"k":"z"}"#,
    r#"{"c":0,"d":"","n":10,"s":""}"#,
    r#"{"c":true}"#,
    r#"{"d":true,"n":11,"s":"x"}"#,
    "not json",
];

/// How much of the grid runs (CLAUDE.md, "Heavy tests"), from `HOLON_GRIDS`: the full set runs
/// every handler on every request; the small set the route's on `SMALL_REQUESTS` alone, and a
/// clock's and a queue's on none. Either checks every handler and writes each back, whole: only
/// runs are left out. Unset is the full set, so a grid run by hand misses nothing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum GridSize {
    Full,
    Small,
}

/// Each size by the name `HOLON_GRIDS` gives it (.github/what-changed.sh writes them, and
/// tests/heavy.rs holds that it writes these and no other).
pub(super) const GRID_SIZES: &[(&str, GridSize)] = &[("full", GridSize::Full), ("small", GridSize::Small)];

pub(super) fn grid_size() -> GridSize {
    let Ok(name) = std::env::var("HOLON_GRIDS") else { return GridSize::Full };
    GRID_SIZES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s).unwrap_or_else(|| panic!("HOLON_GRIDS={name}: the grids know {}", GRID_SIZES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" and ")))
}

/// The requests (of `REQUESTS`, from 0) the small set runs the route's grid on: none of the
/// conditions holding, and a body that isn't JSON. Between them every kind of finding the full set
/// shows, shows (measured 2026-11-23); the full set checks so every time it runs, on its own runs
/// on these requests, and fails naming a kind they lose.
const SMALL_REQUESTS: &[usize] = &[1, 7];

/// The requests (of `REQUESTS`) `block`'s grid runs at `size`.
fn grid_requests(block: Block, size: GridSize) -> Vec<usize> {
    match (size, block) {
        (GridSize::Full, _) => (0..REQUESTS.len()).collect(),
        (GridSize::Small, Block::Route) => SMALL_REQUESTS.to_vec(),
        (GridSize::Small, _) => Vec::new(),
    }
}

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
    ("after && in an if", "if (({C}) && {V}) {\n++state.o\n}\nreturn \"end\""),
    ("after &&", "const got = ({C}) && {V}\nreturn \"end\""),
    ("after || in an if", "if (({C}) || {V}) {\n++state.o\n}\nreturn \"end\""),
    ("after ||", "const got = ({C}) || {V}\nreturn \"end\""),
    ("after ??", "const got = req.json.v ?? {V}\nreturn \"end\""),
    ("after ?? in an if", "if ({C}) {\nconst got = req.json.v ?? {V}\n}\nreturn \"end\""),
    ("after ?? after a return", "if ({C}) return \"stopped\"\nconst got = req.json.v ?? {V}\nreturn \"end\""),
    ("both sides of ?? in an if", "if ({C}) {\nconst got = {V} ?? {V}\n}\nreturn \"end\""),
    ("a branch of ?: in a condition", "if ({C} ? {V} : undefined) {\n++state.o\n}\nreturn \"end\""),
    ("a branch of ?: in an if", "if (req.json.d) {\nconst got = {C} ? {V} : undefined\n}\nreturn \"end\""),
    ("bound, and read in a branch", "const got = {V}\nreturn {C} ? got : \"no\""),
    ("tied to a value there", "let got\nif (req.json.v !== undefined) got = {V}\nreturn \"end\""),
    ("returned from a case", "switch (req.json.k) {\ncase \"a\":\nreturn {V}\ndefault:\nreturn \"no\"\n}"),
    ("in a try", "try {\nconst parsed = JSON.parse(req.body)\n{E}\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
    ("in an if in a try", "try {\nconst parsed = JSON.parse(req.body)\nif ({C}) {\n{E}\n}\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
    ("in a try in an if", "if ({C}) {\ntry {\nconst parsed = JSON.parse(req.body)\n{E}\n} catch {\nreturn \"bad\"\n}\n}\nreturn \"end\""),
    ("in a catch in an if", "if ({C}) {\ntry {\nconst parsed = JSON.parse(req.body)\n} catch {\n{E}\n}\n}\nreturn \"end\""),
    ("before a failure in a try", "try {\n{E}\nconst parsed = JSON.parse(req.body)\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
    ("before a throw in a try", "try {\n{E}\nif ({C}) throw new Error(\"no\")\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
    ("after a throw in a try", "try {\nif ({C}) throw new Error(\"no\")\n{E}\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
    ("bound before a failure in a try", "try {\nconst got = {V}\nconst parsed = JSON.parse(req.body)\n} catch {\nreturn \"bad\"\n}\nreturn \"end\""),
    ("returned after a failure in a try", "try {\nconst got = {V}\nconst parsed = JSON.parse(req.body)\nreturn got\n} catch {\nreturn \"bad\"\n}"),
    ("in a catch", "try {\nconst parsed = JSON.parse(req.body)\n} catch {\n{E}\n}\nreturn \"end\""),
    ("in an if on a value there", "if (req.json.v !== undefined) {\n{E}\n}\nreturn \"end\""),
    ("after a throw, no try", "if ({C}) throw new Error(\"no\")\n{E}\nreturn \"end\""),
    ("after a failure, no try", "const parsed = JSON.parse(req.body)\n{E}\nreturn \"end\""),
    ("in a finally", "try {\nconst parsed = JSON.parse(req.body)\n} catch {\nreturn \"bad\"\n} finally {\n{E}\n}\nreturn \"end\""),
    ("in a finally, a return above", "if ({C}) return \"stopped\"\ntry {\nconst parsed = JSON.parse(req.body)\n} catch {\nreturn \"bad\"\n} finally {\n{E}\n}\nreturn \"end\""),
    ("in a try without a catch, a return above", "if ({C}) return \"stopped\"\ntry {\n{E}\n} finally {\n++state.o\n}\nreturn \"end\""),
    ("a branch of ?:, read only in a branch", "const got = {C} ? {V} : undefined\nreturn req.json.d ? got : \"no\""),
    ("a branch of ?: past a return", "if (req.json.d) return \"stopped\"\nconst got = {C} ? {V} : undefined\nreturn \"end\""),
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
            for (condition, read, as_value) in conditions {
                let fill = |c: &str| template.replace("{C}", c).replace("{E}", statement).replace("{V}", value);
                // The right of `&&` and `||` is run only when the left doesn't decide.
                let read = match *shape {
                    "after &&" => format!("const got = ANDL({as_value}, () => {value})\nreturn \"end\""),
                    "after ||" => format!("const got = ORL({as_value}, () => {value})\nreturn \"end\""),
                    "after && in an if" => format!("if (ANDC({read}, () => {value})) {{\n++state.o\n}}\nreturn \"end\""),
                    "after || in an if" => format!("if (ORC({read}, () => {value})) {{\n++state.o\n}}\nreturn \"end\""),
                    _ => fill(read),
                };
                // A subject that isn't there takes no case, the default no more than another.
                let read = read.replace("req.json.", "J.").replace("switch (J.k) {", "if (J.k !== undefined) switch (J.k) {");
                // A try block that fails leaves nothing behind but the requests it made.
                let read = read.replace("try {\n", "const TM = mark()\ntry {\n").replace("} catch {\n", "} catch {\nundo(TM)\n");
                // Outside a try, a throw ends the handler as a return of nothing (§6.5a); a line
                // that fails ends it too, as JavaScript's throw does (the owner's open question:
                // a flow goes on today, owed under its debt).
                // A throw's message goes to the run's log wherever the throw is reached (`boom`).
                let read = read.replace("throw new Error(\"boom\")", "{ boom(); throw new Error(\"boom\") }");
                let read = match *shape {
                    "after a throw, no try" => read.replace("throw new Error(\"no\")", "return"),
                    "after a failure, no try" => read.replace("const parsed = JSON.parse(req.body)\n", "const parsed = PARSED(req.body)\nif (parsed === undefined) return\n"),
                    _ => read,
                };
                out.push(Case { shape, mark, condition, body: fill(condition), read: neither(&read) });
            }
        }
    }
    out
}

/// `read`'s `if`s as a flow reads them: on a condition that isn't there neither branch runs, and
/// if one of them returns nothing after the `if` does either (docs/LANGUAGE.md §6.4), where
/// JavaScript takes the else. Each `if (x)` is written `if ((IFC = x) === undefined) {} else if
/// (IFC)`, with `return;` in place of `{}` where a branch returns.
pub(super) fn neither(read: &str) -> String {
    let mut heads = Vec::new();
    let mut at = 0;
    while let Some(i) = read[at..].find("if (").map(|i| i + at) {
        if read[..i].chars().last().is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            let close = matching(read, i + 3);
            let branch = &read[close + 1..if_end(read, i)];
            // In a try block, a condition that isn't there is a value that didn't arrive: the block
            // fails and its catch runs (§6.5, "catch catches absence").
            let in_try = read[..i].match_indices("try {").any(|(j, _)| i < matching(read, j + 4));
            heads.push((i, close, in_try, branch.contains("return") || branch.contains("throw")));
        }
        at = i + 4;
    }
    let mut out = read.to_string();
    for (i, close, in_try, returns) in heads.into_iter().rev() {
        let stop = if in_try {
            "throw 0;"
        } else if returns {
            "return;"
        } else {
            "{}"
        };
        out.replace_range(i..=close, &format!("if ((IFC = {}) === undefined) {stop} else if (IFC)", &read[i + 4..close]));
    }
    out
}

/// Where the bracket opened at `open` closes, texts in double quotes skipped.
fn matching(s: &str, open: usize) -> usize {
    let (o, c) = if s.as_bytes()[open] == b'(' { (b'(', b')') } else { (b'{', b'}') };
    let (mut depth, mut in_text) = (0, false);
    for (k, &ch) in s.as_bytes().iter().enumerate().skip(open) {
        if ch == b'"' {
            in_text = !in_text;
        } else if !in_text && ch == o {
            depth += 1;
        } else if !in_text && ch == c {
            depth -= 1;
            if depth == 0 {
                return k;
            }
        }
    }
    panic!("unbalanced at {open}: {s}")
}

/// Where the `if` at `i` ends: its branch, and its else's.
fn if_end(s: &str, i: usize) -> usize {
    let end = branch_end(s, matching(s, i + 3) + 1);
    match s[end..].strip_prefix(" else ") {
        Some(rest) if rest.starts_with("if (") => if_end(s, s.len() - rest.len()),
        Some(rest) => branch_end(s, s.len() - rest.len()),
        None => end,
    }
}

/// Where a branch starting at `at` ends: a block, or a statement at the end of its line.
fn branch_end(s: &str, at: usize) -> usize {
    let at = at + s[at..].len() - s[at..].trim_start_matches(' ').len();
    if s.as_bytes().get(at) == Some(&b'{') { matching(s, at) + 1 } else { s[at..].find('\n').map_or(s.len(), |n| at + n) }
}

/// The kind of block a grid's handlers are: a route, a clock's, a queue's, a file's, a mail's or a
/// feed's handler, or a websocket's `message` block. A handler that isn't a route's or a
/// websocket's has no request: it reads the grid's request, `{json, body}`, from the state (set by
/// a route `POST /set` before each run: a clock's, a file's, a mail's) or from its message's or
/// event's data (a queue's, a feed's). A websocket's message is its request, its text the body.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Block {
    Route,
    Every,
    Queue,
    File,
    Mail,
    Feed,
    Socket,
}

impl Block {
    /// The name a handler's source is imported under in a flow's text, `<name><j>` for its `j`th
    /// handler, for a block whose source one handler alone listens to; bound as `w<w><name><j>`
    /// for the worker `w` that runs it (one deployment listens to it, so each worker's flow is
    /// removed before its next is deployed). Its kind of binding.
    fn source(self) -> Option<(&'static str, &'static str)> {
        match self {
            Block::Queue => Some(("q", "queue")),
            Block::Mail => Some(("mb", "mail")),
            Block::Feed => Some(("fd", "feed")),
            _ => None,
        }
    }

    /// Whether its run answers nobody, so its returns are bare (`answers_nobody`).
    fn answers_nobody(self) -> bool {
        !matches!(self, Block::Route | Block::Socket)
    }

    /// Whether its handlers read the grid's request from the state, set by `POST /set`.
    fn reads_the_state(self) -> bool {
        matches!(self, Block::Every | Block::File | Block::Mail)
    }
}

/// The sources a grid's flow listens to, one a handler: `q0` to `q39` in its text, and so on.
const GRID_QUEUES: usize = 40;

/// The bound name of the `j`th source of `block` of worker `w`'s flows.
fn grid_source(block: Block, w: usize, j: usize) -> String {
    format!("w{w}{}{j}", block.source().map_or("", |(n, _)| n))
}

/// The folder a file grid's handlers watch, each for its own file `r<j>.txt`.
const GRID_FOLDER: &str = "inbox";

/// How many flows of a grid run side by side.
fn grid_workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8))
}

/// A flow of `handlers`, each a route `POST /r<i>`, or the `j`th a clock ticking every `j+1`
/// seconds or the handler of the queue `q<j>`: only what they use is imported and defined.
fn flow_text(name: &str, port: u16, host: &str, handlers: &[(usize, &Case)], block: Block) -> String {
    let uses = |what: &str| handlers.iter().any(|(_, c)| c.body.contains(what));
    let starts = uses("ping(") || uses("go(\"") || uses("maybe(");
    let blocks = match block {
        Block::Route | Block::Socket => "",
        Block::Every => "every, ",
        Block::Queue | Block::File | Block::Mail | Block::Feed => "on, ",
    };
    let mut text = format!("import {{ {}{blocks}flow, route{} }} from \"holon\"\nimport state from \"state\"\n", if starts { "action, " } else { "" }, if uses("status(") { ", status" } else { "" });
    if block == Block::Every {
        text.push_str("import { now } from \"holon:clock\"\n");
    }
    if let Some((name, kind)) = block.source() {
        for j in 0..handlers.len() {
            text.push_str(&format!("import {name}{j} from \"{kind}:{name}{j}\"\n"));
        }
    }
    if block == Block::File {
        text.push_str(&format!("import {GRID_FOLDER} from \"folder:{GRID_FOLDER}\"\n"));
    }
    if uses("marks.") || uses("marks[") || uses("part(") || uses("held(") || uses("every") {
        text.push_str("import marks from \"collection:marks\"\n");
    }
    if uses("jobs.") || uses("every") {
        text.push_str("import jobs from \"queue:jobs\"\n");
    }
    if uses("ops.") || uses("every") {
        text.push_str("import ops from \"mail:ops\"\n");
    }
    if uses("notes[") {
        text.push_str("import notes from \"collection:notes\"\n");
    }
    if uses("gone[") {
        text.push_str("import gone from \"collection:gone\"\n");
    }
    if uses("files[") || uses("files.") {
        text.push_str("import files from \"folder:files\"\n");
    }
    if uses("shop.") {
        text.push_str("import shop from \"database:shop\"\n");
    }
    if uses("hail(") {
        text.push_str("import { hail } from \"flow:callee\"\n");
    }
    if uses("news.") || uses("held(") || uses("every") {
        text.push_str("import news from \"channel:news\"\n");
    }
    if uses("api.") || uses("ask(") || uses("ask2(") || uses("askif(") {
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
    if uses("askif(") {
        text.push_str("\nasync function askif(x) {\n  if (x) {\n    const sent = await api.post(\"/in\", { body: x })\n  }\n  return 1\n}\n");
    }
    if uses("every") {
        text.push_str("\nfunction every(x) {\n  ++state.f\n  marks.put(\"k\", x)\n  news.send({ n: x })\n  jobs.send({ data: { n: x } })\n  ops.send({ to: \"a@acme.com\", subject: x, text: \"hi\" })\n  return `did ${x}`\n}\n");
    }
    if uses("every2(") {
        text.push_str("\nfunction every2(x) {\n  const did = every(x)\n  return `${did} twice`\n}\n");
    }
    if uses("check(") {
        text.push_str("\nfunction check(x) {\n  return x === \"a\"\n}\n");
    }
    if uses("bump(") {
        text.push_str("\nfunction bump(x) {\n  ++state.f\n  return x\n}\n");
    }
    if uses("go(\"") {
        text.push_str("\nfunction go(x) {\n  ping({})\n  return `went ${x}`\n}\n");
    }
    if uses("maybe(") {
        text.push_str("\nfunction maybe(x) {\n  return x ? ping(x) : 0\n}\n");
    }
    if uses("fin(") {
        text.push_str("\nfunction fin(x) {\n  if (x === \"a\") return \"skipped\"\n  try {\n    const parsed = JSON.parse(x)\n  } catch {\n    return \"kept\"\n  } finally {\n    ++state.f\n  }\n  return \"parsed\"\n}\n");
    }
    if uses("held(") {
        text.push_str("\nfunction held(x) {\n  try {\n    ++state.f\n    marks.put(\"k\", x)\n    const parsed = JSON.parse(x)\n    return \"parsed\"\n  } catch {\n    news.send({ n: x })\n    return \"kept\"\n  }\n}\n");
    }
    if starts {
        text.push_str("\nexport const ping = action(() => ++state.pinged)\n");
    }
    if block.reads_the_state() {
        text.push_str("\nroute.post(\"/set\", (req) => {\n  state.req = req.json\n  return \"ok\"\n})\n");
    }
    for (j, (i, case)) in handlers.iter().enumerate() {
        let r#async = if case.body.contains("await ") { "async " } else { "" };
        match block {
            Block::Route => text.push_str(&format!("\nroute.post(\"/r{i}\", {async}(req) => {{\n{}\n}})\n", case.body)),
            // A clock's handler is what is wired to its tick (`now`): the request read from the
            // state is tied to it, and so all that's computed from it. A file's and a mail's are
            // tied to what started them the same way.
            Block::Every => text.push_str(&format!("\nevery(\"{}s\", {async}() => {{\nconst req = now > 0 ? state.req : undefined\n{}\n}})\n", j + 1, case.body)),
            Block::Queue => text.push_str(&format!("\non.queue(q{j}, {async}(m) => {{\nconst req = m.data\n{}\n}})\n", case.body)),
            Block::Feed => text.push_str(&format!("\non.feed(fd{j}, {async}(e) => {{\nconst req = e.data\n{}\n}})\n", case.body)),
            Block::Mail => text.push_str(&format!("\non.mail(mb{j}, {async}(m) => {{\nconst req = m.subject ? state.req : undefined\n{}\n}})\n", case.body)),
            Block::File => text.push_str(&format!("\non.file.created({GRID_FOLDER}, {async}(f) => {{\nconst req = f.size >= 0 ? state.req : undefined\n{}\n}}, {{ matching: \"r{j}.txt\" }})\n", case.body)),
            // A websocket's message is its request: its text is the body the grid reads.
            Block::Socket => text.push_str(&format!("\nroute.websocket(\"/r{i}\", {{\nmessage: {async}(req) => {{\n{}\n}},\n}})\n", case.body.replace("req.body", "req.text"))),
        }
    }
    text
}

/// A handler's body for a block whose run answers nobody (a clock's, a queue's): `return x` ends
/// it bare. An answer that is a text or a status goes; one that computes (`return got`, `return
/// c ? got : "no"`, `return f()`) is bound first (`const out1 = …`), so what it reads or calls is
/// still read or called, in JavaScript and in the flow alike.
fn answers_nobody(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    let mut n = 0;
    while let Some(at) = rest.find("return ") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "return ".len()..];
        let end = after.find('\n').unwrap_or(after.len());
        let value = &after[..end];
        if value.starts_with('"') || value.starts_with("status(") {
            out.push_str("return");
        } else {
            n += 1;
            out.push_str(&format!("const out{n} = {value}\nreturn"));
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The grid's request as a clock's or a queue's handler reads it: `{json, body}`, `json` left out
/// where the body isn't JSON.
fn envelope(body: &str) -> serde_json::Value {
    let mut e = serde_json::json!({ "body": body });
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        e["json"] = json;
    }
    e
}

/// One run of the `j`th handler of `block` on `body`: a request to its route, a tick of its
/// clock after its request is set, or a message on its queue.
fn run_one(state: &AppState, slug: &str, block: Block, (i, j, w): (usize, usize, usize), body: &str) {
    match block {
        Block::Route => {
            data_call(state, slug, "POST", &format!("/r{i}"), body);
        }
        Block::Every => {
            let (status, said, _) = data_call(state, slug, "POST", "/set", &envelope(body).to_string());
            assert_eq!(status, 200, "{said}");
            let r = route(state, &req("POST", &format!("/deployments/{slug}/tick"), serde_json::to_vec(&serde_json::json!({ "clock": format!("every {}s", j + 1) })).unwrap()));
            let status = r.status_code;
            assert_eq!(status, 200, "tick {}: {}", j + 1, String::from_utf8_lossy(&body_bytes(r)));
        }
        Block::Queue => {
            let data = holon_core::parse_json(&envelope(body).to_string()).unwrap();
            let queue = grid_source(block, w, j);
            assert!(dequeue::fire_for_test(&state.store, &state.deployments, &state.deployments_path, slug, &queue, data), "{queue} of {slug} didn't run");
        }
        Block::Feed => {
            let data = holon_core::parse_json(&envelope(body).to_string()).unwrap();
            let feed = grid_source(block, w, j);
            assert!(subscriptions::fire_for_test(&state.store, &state.deployments, &state.deployments_path, slug, &feed, data), "{feed} of {slug} didn't run");
        }
        Block::File | Block::Mail => {
            let (status, said, _) = data_call(state, slug, "POST", "/set", &envelope(body).to_string());
            assert_eq!(status, 200, "{said}");
            if block == Block::File {
                assert!(arrivals::fire_for_test(&state.store, &state.deployments, &state.deployments_path, slug, GRID_FOLDER, &format!("r{j}.txt")), "r{j}.txt of {slug} ran nothing");
            } else {
                let message = holon_core::parse_json(r#"{"from":"a@acme.com","name":"A","to":["ops@acme.com"],"cc":[],"subject":"s","date":0,"message_id":"<m@acme.com>","text":"hi","attachments":[],"authenticated":true,"verdict":"pass"}"#).unwrap();
                let mail = grid_source(block, w, j);
                assert!(inbox::fire_for_test(&state.store, &state.deployments, &state.deployments_path, slug, &mail, message), "{mail} of {slug} didn't run");
            }
        }
        Block::Socket => {
            let port = state.deployments.lock().unwrap()[slug].server.as_ref().and_then(|s| s.port).expect("a websocket's flow has a port");
            let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.set_read_timeout(Some(EVENTUALLY)).unwrap();
            use tungstenite::client::IntoClientRequest;
            let (mut ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/r{i}").into_client_request().unwrap(), stream).unwrap_or_else(|e| panic!("/r{i} of {slug}: {e}"));
            ws.send(tungstenite::Message::text(body)).unwrap();
            // Its run is logged before its answer is sent, if it is sent: the log is read until
            // it gains it (`flow_does`), and then the connection closes.
            SOCKETS.with(|s| s.borrow_mut().push(ws));
        }
    }
}

thread_local! {
    /// A websocket grid's connections, each held until its run is read from the log.
    static SOCKETS: std::cell::RefCell<Vec<tungstenite::WebSocket<std::net::TcpStream>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// What a way of running a handler did: the marks it left, in order of their letters, and
/// whether it threw.
#[derive(Clone, Debug, PartialEq)]
struct Did {
    marks: String,
    threw: bool,
}

/// What JavaScript does with each of `handlers` on each of `requests` (of `REQUESTS`), as written
/// and as a flow reads it. The names a handler uses are stand-ins that leave their mark; `await`
/// is dropped, since they answer at once.
fn javascript_does(handlers: &[&Case], requests: &[usize]) -> Vec<Vec<(Did, Did)>> {
    let names = "state, marks, console, ping, rec, rec2, rec3, part, ask, ask2, api, status, mark, undo, news, gone, bump, go, maybe, askif, held, fin, notes, check, jobs, ops, every, every2, files, shop, hail, boom";
    let made: Vec<String> = handlers
        .iter()
        .map(|c| {
            let (written, read) = (c.body.replace("await ", ""), c.read.replace("await ", ""));
            format!("[({{ {names} }}) => (req) => {{\n{written}\n}}, ({{ {names} }}) => (req) => {{\nconst J = req.json ?? {{}};\nlet IFC;\n{read}\n}}]")
        })
        .collect();
    let source = format!(
        r#"(requests) => {{
  {AS_A_FLOW_READS}
  const handlers = [
{}
  ];
  const run = (make, req, promise) => {{
    const fx = [];
    const state = {{}};
    const env = {{
      state,
      marks: new Proxy({{ put: () => {{ fx.push("p") }} }}, {{ set: () => {{ fx.push("p"); return true }} }}),
      // A read of a key: it answers nothing. A key that isn't there (`notes[req.json.k]` on a
      // request without `k`, which JavaScript reads as the key "undefined") is no read, on both
      // sides alike (as written and as a flow reads it): a flow asks no read with a key it hasn't
      // got, and counting JavaScript's read of "undefined" would show a difference that is no
      // question of reached, only of how the stand-in is keyed.
      notes: new Proxy({{}}, {{ get: (t, k) => {{ if (typeof k === "string" && k !== "undefined") fx.push("n"); return undefined }} }}),
      check: (x) => x === "a",
      console: {{ log: () => {{ fx.push("l") }} }},
      ping: () => {{ fx.push("s"); return {{ run: "r" }} }},
      rec: (x) => {{ fx.push("f"); return "did " + x }},
      rec2: (x) => {{ fx.push("f"); return "did " + x + " twice" }},
      rec3: (x) => {{ fx.push("f"); return "did " + x + " twice, thrice" }},
      part: (x) => {{ fx.push("f"); fx.push("p"); return {{ did: "did " + x }} }},
      ask: () => {{ fx.push("g"); return 200 }},
      ask2: () => {{ fx.push("g"); return 200 }},
      api: {{
        post: (path) => {{ fx.push(path === "/st" ? "t" : "r"); return {{ status: 200, body: "fast" }} }},
        put: () => {{ fx.push("u"); return {{ status: 200, body: "fast" }} }},
      }},
      files: new Proxy({{ file: (name) => ({{ name }}) }}, {{ set: () => {{ fx.push("h"); return true }} }}),
      shop: {{ exec: () => {{ fx.push("b"); return 1 }} }},
      hail: () => {{ fx.push("k"); return {{ answer: 1 }} }},
      boom: () => {{ fx.push("x") }},
      status: (n) => ({{ status: n }}),
      news: {{ send: () => {{ fx.push("c") }} }},
      jobs: {{ send: () => {{ fx.push("q") }} }},
      ops: {{ send: () => {{ fx.push("m") }} }},
      every: (x) => {{ fx.push("f", "p", "c", "q", "m"); return "did " + x }},
      every2: (x) => {{ fx.push("f", "p", "c", "q", "m"); return "did " + x + " twice" }},
      gone: new Proxy({{}}, {{ deleteProperty: () => {{ fx.push("d"); return true }} }}),
      bump: (x) => {{ fx.push("f"); return x }},
      go: (x) => {{ fx.push("s"); return "went " + x }},
      maybe: (x) => {{ if (x) fx.push("s"); return x ? {{ run: "r" }} : 0 }},
      askif: (x) => {{ if (x) fx.push("g"); return 1 }},
      // Its block writes and puts before it fails: JavaScript keeps both, the try's promise
      // neither.
      held: (x) => {{
        let ok = true;
        try {{ JSON.parse(x) }} catch (e) {{ ok = false }}
        if (ok || !promise) fx.push("f", "p");
        if (!ok) fx.push("c");
        return ok ? "parsed" : "kept";
      }},
      // Its finally runs where its try was entered: not past the return before it.
      fin: (x) => {{
        if (x === "a") return "skipped";
        fx.push("f");
        let ok = true;
        try {{ JSON.parse(x) }} catch (e) {{ ok = false }}
        return ok ? "parsed" : "kept";
      }},
      // The try's promise (§6.5): what a failed block made is undone, but for its requests and
      // the reads it asked.
      mark: () => [fx.length, Object.assign({{}}, state)],
      undo: ([n, kept]) => {{
        fx.push(...fx.splice(n).filter((m) => m === "r" || m === "g" || m === "n" || m === "u" || m === "t" || m === "x"));
        for (const k of Object.keys(state)) delete state[k];
        Object.assign(state, kept);
      }},
    }};
    let threw = false;
    // As a flow reads it, a throw outside a try ends the handler as a return of nothing.
    try {{ make(env)(req) }} catch (e) {{ threw = !(promise && String(e).includes("boom")) }}
    for (const k of Object.keys(state)) fx.push(k);
    return {{ marks: fx.sort().join(""), threw }};
  }};
  return handlers.map(([written, read]) => requests.map((req) => [run(written, req, false), run(read, req, true)]));
}}"#,
        made.join(",\n")
    );
    let requests: Vec<Value> = requests
        .iter()
        .map(|&r| REQUESTS[r])
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
fn flow_does(state: &AppState, slug: &str, block: Block, at: (usize, usize, usize), body: &str, seen: &mut Option<u64>) -> String {
    run_one(state, slug, block, at, body);
    let after = seen.map(|seq| format!("after={seq}&")).unwrap_or_default();
    let read = || body_json(route(state, &req("GET", &format!("/deployments/{slug}/log?{after}limit=50"), vec![])));
    let mut gained = read();
    // A websocket's message runs on the server's own thread: its log is read until it gains it.
    if block == Block::Socket {
        let ran = |g: &serde_json::Value| g["entries"].as_array().is_some_and(|e| e.iter().any(|e| e["kind"] == "websocket"));
        assert!(soon(|| ran(&read())), "a websocket message to /r{} of {slug} wasn't run", at.0);
        gained = read();
        SOCKETS.with(|s| s.borrow_mut().drain(..).for_each(|mut ws| drop(ws.close(None))));
    }
    let entries = gained["entries"].as_array().unwrap();
    let mut marks: Vec<&str> = Vec::new();
    for e in entries {
        *seen = (*seen).max(e["seq"].as_u64());
        if e["route"] == "action ping" {
            marks.push("s");
        }
        // The handler's own run: a request, a tick or a queue's, but not the request that set a
        // clock's request.
        if !matches!(e["kind"].as_str(), Some("request" | "tick" | "queue" | "file" | "mail" | "feed" | "websocket")) || e["route"] == "POST /set" {
            continue;
        }
        for p in e["performed"].as_array().into_iter().flatten() {
            match p["capability"].as_str().unwrap_or("") {
                "StateWrite(\"w\")" => marks.push("w"),
                "StateWrite(\"o\")" => marks.push("o"),
                "StateWrite(\"f\")" => marks.push("f"),
                "CollectionWrite(\"marks\")" => marks.push("p"),
                "CollectionWrite(\"gone\")" => marks.push("d"),
                "ChannelSend(\"news\")" => marks.push("c"),
                "QueueSend(\"jobs\")" => marks.push("q"),
                "MailSend(\"ops\")" => marks.push("m"),
                "FileWrite(\"files\")" => marks.push("h"),
                other if other.starts_with("DbWrite(\"shop") => marks.push("b"),
                other => panic!("an effect the grid doesn't know: {other}"),
            }
        }
        // A read the run asked of `notes`, from its record.
        for r in e["reads"].as_array().into_iter().flatten() {
            if r["request"].to_string().contains("\"notes\"") {
                marks.push("n");
            }
        }
        let sent = serde_json::to_string(&e["rounds"]).unwrap();
        if sent.contains("/hit\"") {
            marks.push("r");
        }
        if sent.contains("/in\"") {
            marks.push("g");
        }
        if sent.contains("/up\"") {
            marks.push("u");
        }
        if sent.contains("/st\"") {
            marks.push("t");
        }
        if sent.contains("hail") {
            marks.push("k");
        }
        // A line is no effect a run performed: it is kept beside the runs, under its run's seq.
        let lines = body_json(route(state, &req("GET", &format!("/deployments/{slug}/logs?run={}", e["seq"]), vec![])));
        if lines["lines"].as_array().is_some_and(|l| !l.is_empty()) {
            marks.push("l");
        }
        // A throw's message is the reason its step gave nothing, on the run's timeline (`x`).
        if e["timeline"].as_array().into_iter().flatten().any(|t| t["reason"].as_str().is_some_and(|r| r.contains("boom"))) {
            marks.push("x");
        }
    }
    marks.sort();
    marks.concat()
}

/// Where a flow doesn't yet do what it reads as, and why: debts, each to be paid by a change to
/// the language and then struck from here. A difference outside these fails the test, and so does
/// one of these that no longer differs anywhere. The debts are reached's (`reached.rs`, one smallest flow each), and the started half, carried
/// from the try's own work.
const DEBTS: &[&Debt] = &[
    &A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE,
    &THE_STARTED_HALF,
    &A_FINALLY_FORGETS_THE_RETURN_ABOVE,
    &AN_ARMS_CALL_READ_ONLY_ELSEWHERE_IS_NOT_MADE,
    &A_LINE_AFTER_A_FAILURE_RUNS,
    &A_FUNCTIONS_OWN_CONDITION,
    &A_LISTS_FUNCTION_STARTS_PAST_A_RETURN,
    &A_LISTS_FUNCTION_LOSES_ITS_WRITE,
    &A_CHOICE_IN_A_FUNCTION_STARTS,
    &A_CALLED_START_READ_ONLY_ELSEWHERE_IS_NOT_MADE,
    &A_CHOICES_TEST_IS_READ_PAST_A_RETURN,
    &A_READ_IS_ASKED_PAST_A_RETURN,
    &AN_UNUSED_READ_IS_NOT_ASKED,
];

/// The started half: a start in a try block that then fails is made, as JavaScript makes it,
/// where the try's promise keeps nothing of a failed block but its requests (the started half's
/// own design, open).
pub(super) const THE_STARTED_HALF: Debt = Debt {
    name: "the started half",
    says: "an action a try block starts, bound to a name or through a function, or another flow's action it calls, is started though the block then fails",
};

/// The shapes whose try block fails on some request, the statement in it reached before.
const FAILING_TRIES: &[&str] = &["before a failure in a try", "bound before a failure in a try", "returned after a failure in a try", "before a throw in a try"];

/// Where the compiler refuses a handler with a fault of its own (a graph that can't run), known and
/// owned elsewhere: by shape and effect, and why. Each is left out as a refusal is, and one that
/// compiles again fails the test, to be struck from here.
const KNOWN_FAULTS: &[(&str, &str, &str)] = &[
    (
        "tied to a value there",
        "ls",
        "a list's function that starts an action, bound under a condition (`if (v) got = [\"a\"].map((x) => ping(x))`): \"`after` ties a write or a call to what it names: this statement makes none\" (R174)",
    ),
    (
        "tied to a value there",
        "rd",
        "a collection read bound under a condition (`if (v !== undefined) got = notes[\"k\"]`): \"`after` ties a write or a call to what it names: this statement makes none\" (found 2026-11-22 by the reads column)",
    ),
];

/// Handlers the reader takes and `lang::compile` refuses, as their steps written back as text read
/// as other steps: each debt, the block whose handlers alone owe it (none: any block's, held by
/// the route's grid), and the cases owed under it. Paid when the printer writes the form as the
/// same steps; then the debt fails the test until it is struck.
type Owes = fn(&Case) -> bool;
const ROUND_TRIP_DEBTS: &[(&str, Option<Block>, Owes)] = &[
    // Found by the websocket grid (2026-11-23): a message block whose answer, returned from a
    // case, is a start, a request, an upload or another flow's action called.
    ("a websocket's message returning a call from a case is written back as another flow", Some(Block::Socket), |c| {
        c.shape == "returned from a case" && matches!(c.mark, "s" | "r" | "u" | "k")
    }),
    ("the printer writes && / || around an awaited function as other steps", None, |c| matches!(c.shape, "after &&" | "after ||") && c.mark == "g" && c.condition == "req.json.c"),
    // Found by the grid's new columns (2026-11-22): the same shape, a function with a try block
    // of its own.
    ("the printer writes && / || around a function with a try of its own as other steps", None, |c| {
        matches!(c.shape, "after &&" | "after ||") && matches!(c.mark, "ft" | "fu" | "fy") && c.condition == "req.json.c"
    }),
    // Found by the `every` columns (2026-11-22): a function that writes state, puts, sends, queues
    // and mails is refused everywhere: "state.set(f, …)'s answer is used, which TypeScript can't
    // say".
    ("the printer can't write back a function that writes state and then sends, queues and mails", None, |c| matches!(c.mark, "fa" | "fa2")),
    // Reached's debts whose smallest flow the round trip refuses (reached.rs, `Today::Refused`).
    (A_FUNCTIONS_OWN_CONDITION.name, None, |c| c.mark == "fs"),
    (A_LISTS_FUNCTION_LOSES_ITS_WRITE.name, None, |c| c.mark == "lf"),
    (A_LISTS_FUNCTION_STARTS_PAST_A_RETURN.name, None, |c| c.mark == "ls"),
    (A_CHOICE_IN_A_FUNCTION_STARTS.name, None, |c| c.mark == "fr"),
];

/// The debt a run that differs is owed under, by its case and request and the difference, mark by
/// mark: what the flow made that it reads as not making (`made`), and what it didn't make that it
/// reads as making (`missed`). Each debt names one difference; any other fails.
fn debt(case: &Case, request: &str, made: &str, missed: &str) -> Option<Vec<&'static Debt>> {
    let (mut made, mut missed) = (made.to_string(), missed.to_string());
    let mut owed: Vec<&'static Debt> = Vec::new();
    // Each debt that applies to the cell explains the marks of its kinds the flow made or missed.
    let mut explain = |applies: bool, debt: &'static Debt, made_kinds: &str, missed_kinds: &str| {
        if !applies {
            return;
        }
        let (m, x) = (made.len(), missed.len());
        made.retain(|c| !made_kinds.contains(c));
        missed.retain(|c| !missed_kinds.contains(c));
        if (made.len(), missed.len()) != (m, x) {
            owed.push(debt);
        }
    };
    let (shape, mark) = (case.shape, case.mark);
    let failing_try = FAILING_TRIES.contains(&shape);
    // `fin` returns early on "a": its finally's write there is W's; elsewhere, in a try that fails,
    // it is a write the failed try keeps.
    let early = request.contains("\"k\":\"a\"");
    explain(shape == "after a failure, no try" && request == "not json", &A_LINE_AFTER_A_FAILURE_RUNS, "wofpcdgrslqmnhbuktx", "");
    explain(matches!(shape, "in a finally, a return above" | "in a try without a catch, a return above"), &A_FINALLY_FORGETS_THE_RETURN_ABOVE, "wo", "");
    explain(mark == "fy" && (early || !failing_try), &A_FINALLY_FORGETS_THE_RETURN_ABOVE, "f", "");
    explain(failing_try && matches!(mark, "f" | "f2" | "f3" | "fp" | "fu" | "ft" | "fy" | "fa" | "fa2"), &A_FAILED_TRY_KEEPS_A_FUNCTIONS_WRITE, "fpcqm", "");
    // Another flow's action called (`k`) is started as an action is: the started half's too.
    explain(failing_try && matches!(mark, "s" | "fs" | "fr" | "k"), &THE_STARTED_HALF, "sk", "");
    explain(shape == "a branch of ?:, read only in a branch" && matches!(mark, "s" | "r" | "u" | "t" | "k"), &AN_ARMS_CALL_READ_ONLY_ELSEWHERE_IS_NOT_MADE, "", "srutk");
    explain(matches!(shape, "bound, and read in a branch" | "a branch of ?:, read only in a branch") && matches!(mark, "ls" | "fr"), &A_CALLED_START_READ_ONLY_ELSEWHERE_IS_NOT_MADE, "", "s");
    explain(matches!(mark, "ft" | "fu" | "gi"), &A_FUNCTIONS_OWN_CONDITION, "cpg", "");
    explain(mark == "ls", &A_LISTS_FUNCTION_STARTS_PAST_A_RETURN, "s", "");
    explain(mark == "lf", &A_LISTS_FUNCTION_LOSES_ITS_WRITE, "", "f");
    explain(mark == "fr", &A_CHOICE_IN_A_FUNCTION_STARTS, "s", "");
    let reads_in_test = case.condition.starts_with("notes[");
    explain(reads_in_test && shape.contains("?:"), &A_CHOICES_TEST_IS_READ_PAST_A_RETURN, "n", "");
    explain(matches!(mark, "rd" | "pk"), &A_READ_IS_ASKED_PAST_A_RETURN, "n", "");
    explain(true, &AN_UNUSED_READ_IS_NOT_ASKED, "", "n");
    (made.is_empty() && missed.is_empty() && !owed.is_empty()).then_some(owed)
}

/// The grid's server: a local host may be called, and a mail (`ops`) and a queue (`jobs`) are
/// bound, to a mail sink and a queue standing in for SQS (the review session's, as it wrote it).
/// The folder its secrets are in, to remove.
pub(super) fn grid_state(sink: &smtp::sink::Sink, sqs: &sqs::stand_in::StandIn) -> (AppState, std::path::PathBuf) {
    grid_state_with(sink, sqs, Block::Route, &[])
}

/// The grid's server, with `more` sources of `block`'s kind, each bound by its name (a queue made
/// on the stand-in first; a mailbox and a feed never read, their runs started by the test), and
/// for a file grid the folder its handlers watch, holding each handler's file.
pub(super) fn grid_state_with(sink: &smtp::sink::Sink, sqs: &sqs::stand_in::StandIn, block: Block, more: &[String]) -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("holon-grid-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    let mut secrets = format!("HOLON_SECRET_OPS_SMTP=smtp://ops:s3cret@127.0.0.1:{}\nHOLON_SECRET_JOBS_URL={}\n", sink.port, sqs.url("jobs"));
    let mut binds = Vec::new();
    // What a mail's and a feed's binding is read from as it is bound: a mailbox and a feed that
    // answer, kept for the grid's length; its runs are started by the test, never read from them.
    let mailbox = (block == Block::Mail).then(|| imap::stand_in::start("grid", "s3cret", true));
    let feed = (block == Block::Feed).then(sse::stand_in::StandIn::start);
    for name in more {
        let secret = format!("HOLON_SECRET_{}_URL", name.to_uppercase());
        match block {
            Block::Queue => {
                sqs.make(name);
                secrets.push_str(&format!("{secret}={}\n", sqs.url(name)));
                binds.push(("/admin/queues", serde_json::json!({ "name": name, "url": format!("env:{secret}") })));
            }
            Block::Mail => {
                secrets.push_str(&format!("{secret}=imap://grid:s3cret@127.0.0.1:{}\n", mailbox.as_ref().unwrap().port));
                binds.push(("/admin/mails", serde_json::json!({ "name": name, "imap": format!("env:{secret}") })));
            }
            Block::Feed => {
                secrets.push_str(&format!("{secret}={}\n", feed.as_ref().unwrap().url("/events")));
                binds.push(("/admin/feeds", serde_json::json!({ "name": name, "url": format!("env:{secret}") })));
            }
            other => panic!("{other:?} has no sources of its own"),
        }
    }
    std::fs::write(&env, secrets).unwrap();
    let sources = || secret_sources::Sources::new(Some(env.clone())).unwrap();
    let mut state = claims_state("");
    let mails = mails::Mails::memory().with_sources(sources());
    let queues = queues::Queues::memory().with_sources(sources());
    let feeds = feeds::Feeds::memory().with_sources(sources());
    state.folders = Arc::new(folders::Folders::memory().with_mails(mails).with_queues(queues).with_feeds(feeds));
    state.folders.mails.serve(Arc::clone(&state.deployments));
    state.folders.queues.serve(Arc::clone(&state.deployments));
    for (path, body) in [
        ("/admin/mails", serde_json::json!({ "name": "ops", "smtp": "env:HOLON_SECRET_OPS_SMTP", "from": "Ops <ops@acme.com>", "to": "*@acme.com" })),
        ("/admin/queues", serde_json::json!({ "name": "jobs", "url": "env:HOLON_SECRET_JOBS_URL" })),
    ] {
        assert_eq!(route(&state, &req("POST", path, serde_json::to_vec(&body).unwrap())).status_code, 200, "{path}");
    }
    // A folder (`files`, holding `a.txt` to send by handle), a database (`shop`) and another flow
    // (`callee`, its action `hail`), for the columns that write, upload and call.
    let files = dir.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("a.txt"), "a file").unwrap();
    for (path, body) in [
        ("/admin/folders", serde_json::json!({ "name": "files", "path": files.display().to_string(), "access": "read-write" })),
        ("/admin/databases", serde_json::json!({ "name": "shop", "path": dir.join("shop.db").display().to_string(), "schema": "create table t (v text)" })),
    ] {
        let bound = route(&state, &req("POST", path, serde_json::to_vec(&body).unwrap()));
        let status = bound.status_code;
        assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&body_bytes(bound)));
    }
    for (path, body) in binds {
        let bound = route(&state, &req("POST", path, serde_json::to_vec(&body).unwrap()));
        let status = bound.status_code;
        assert_eq!(status, 200, "{path} {body}: {}", String::from_utf8_lossy(&body_bytes(bound)));
    }
    if block == Block::File {
        let watched = dir.join(GRID_FOLDER);
        std::fs::create_dir_all(&watched).unwrap();
        for j in 0..GRID_QUEUES {
            std::fs::write(watched.join(format!("r{j}.txt")), "arrived").unwrap();
        }
        let body = serde_json::json!({ "name": GRID_FOLDER, "path": watched.display().to_string(), "access": "read" });
        assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200, "{GRID_FOLDER}");
    }
    if block == Block::Socket {
        state.listeners = Some(Mutex::new(BTreeMap::new()));
    }
    std::mem::forget((mailbox, feed));
    let callee = deploy_text(&state,"import { action, flow } from \"holon\"\n\nflow(\"callee\")\n\nexport const hail = action(() => 1)\n", "127.0.0.1:9");
    assert_eq!(callee, "callee");
    (state, dir)
}

/// What `did` has that `reads` hasn't, and what it lacks, letter by letter.
fn apart(did: &str, reads: &str) -> (String, String) {
    let count = |s: &str| s.chars().fold(BTreeMap::<char, i32>::new(), |mut m, c| {
        *m.entry(c).or_default() += 1;
        m
    });
    let (d, r) = (count(did), count(reads));
    let (mut made, mut missed) = (String::new(), String::new());
    for c in d.keys().chain(r.keys()).collect::<BTreeSet<_>>() {
        let n = d.get(c).copied().unwrap_or(0) - r.get(c).copied().unwrap_or(0);
        let to = if n > 0 { &mut made } else { &mut missed };
        (0..n.abs()).for_each(|_| to.push(*c));
    }
    (made, missed)
}

/// **Guards:** a handler does what its text says. What it writes, sends, starts and calls is what
/// JavaScript would with the same text, but for the one rule a flow has of its own, a value that
/// isn't there (docs/LANGUAGE.md §5.6, and §6.4 for an `if` on one), which is written out here and
/// held exactly.
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
/// Or the compiler refuses a handler it should take: with a graph with a cycle, as it did for a
/// function in a branch of an `if`'s condition (`a ? f() : b`, `a && f()`), or with "`after` ties
/// a write or a call to what it names: this statement makes none", as it did for a call in a
/// branch of a value bound under a condition (`if (d) { const got = v ?? (await api.get(…)).body
/// }`, `c ? ping(x) : undefined` there).
#[test]
#[ignore = "heavy: about 12 minutes, run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_handler_does_what_its_text_says() {
    grid(Block::Route);
}

/// **Guards:** the grid above, every handler a clock's (`every("1s", () => …)`), ticked as the
/// editor's "tick now" ticks it, its request set in the state just before. What a statement
/// reached or not does is the language's, not the route's: a clock's handler does what its text
/// says as a route's does, and differs only where a debt says.
///
/// **If it fails:** as above; a difference only a clock's handler shows is a new debt, named in
/// `reached.rs` with its smallest flow.
#[test]
#[ignore = "heavy: about 8 minutes, run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_clocks_handler_does_what_its_text_says() {
    grid(Block::Every);
}

/// **Guards:** the grid above, every handler a queue's (`on.queue(q, (m) => …)`), run on a message
/// as a message received is run (`dequeue::fire_for_test`, the listener's own run without the
/// queue), its request the message's data.
///
/// **If it fails:** as above.
#[test]
#[ignore = "heavy: about 8 minutes, run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_queues_handler_does_what_its_text_says() {
    grid(Block::Queue);
}

/// **Guards:** the grid above, every handler a file's (`on.file.created(inbox, (f) => …, {
/// matching: "r<j>.txt" })`), run for its file as a look that found it new runs it
/// (`arrivals::fire_for_test`, without the look), its request set in the state just before.
///
/// **If it fails:** as above.
#[test]
#[ignore = "heavy: about 7 minutes (alone, 2026-11-23), run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_files_handler_does_what_its_text_says() {
    grid(Block::File);
}

/// **Guards:** the grid above, every handler a mail's (`on.mail(mb, (m) => …)`), run on a message
/// as one read from its mailbox is run (`inbox::fire_for_test`, without the mailbox), its request
/// set in the state just before.
///
/// **If it fails:** as above.
#[test]
#[ignore = "heavy: about 8 minutes (alone, 2026-11-23), run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_mails_handler_does_what_its_text_says() {
    grid(Block::Mail);
}

/// **Guards:** the grid above, every handler a feed's (`on.feed(fd, (e) => …)`), run on an event as
/// one read from the feed is run (`subscriptions::fire_for_test`, without the feed), its request
/// the event's data.
///
/// **If it fails:** as above.
#[test]
#[ignore = "heavy: about 7 minutes (alone, 2026-11-23), run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_feeds_handler_does_what_its_text_says() {
    grid(Block::Feed);
}

/// **Guards:** the grid above, every handler a websocket's `message` block (`route.websocket("/r",
/// { message: (req) => … })`), run on a message sent through a real socket, its request the
/// message (its text the body).
///
/// **If it fails:** as above.
#[test]
#[ignore = "heavy: about 9 minutes (alone, 2026-11-23), run by .github/workflows/heavy.yml (CLAUDE.md, Heavy tests)"]
fn a_websockets_message_does_what_its_text_says() {
    grid(Block::Socket);
}

/// The grid of `block`'s handlers: every case, checked, run on every request, and compared.
fn grid(block: Block) {
    let size = grid_size();
    let requests = grid_requests(block, size);
    let sink = smtp::sink::start("PLAIN LOGIN");
    let sqs = sqs::stand_in::StandIn::start();
    sqs.make("jobs");
    let workers = grid_workers();
    let sources: Vec<String> = if block.source().is_some() { (0..workers).flat_map(|w| (0..GRID_QUEUES).map(move |j| grid_source(block, w, j))).collect() } else { Vec::new() };
    let (state, secrets) = grid_state_with(&sink, &sqs, block, &sources);
    let (host, _hits) = counting_server();
    // A clock's grid has no `every` function of the flow's (`fa`, `fa2`): `every` is the clock.
    // A clock's or a queue's run answers nobody: its returns are bare (`answers_nobody`).
    let all: Vec<Case> = cases()
        .into_iter()
        .filter(|c| block != Block::Every || !c.mark.starts_with("fa"))
        .map(|c| if !block.answers_nobody() { c } else { Case { body: answers_nobody(&c.body), read: answers_nobody(&c.read), ..c } })
        .collect();
    let flow_text =|name: &str, port: u16, host: &str, handlers: &[(usize, &Case)]| flow_text(name, port, host, handlers, block);
    // Each block's flows on ports of their own. Only a websocket's are listened on, each on a
    // port free as it is deployed.
    let base = match block {
        Block::Route => 19000,
        Block::Every => 20000,
        Block::Queue => 21000,
        Block::File => 22000,
        Block::Mail => 23000,
        Block::Feed => 24000,
        Block::Socket => 25000,
    };
    // What the language refuses is left out: said at check, it is no surprise at a run. A graph
    // with a cycle, or an `after` with nothing to tie, is no refusal of the text but the
    // compiler's own fault, and fails.
    let indexed: Vec<(usize, &Case)> = all.iter().enumerate().collect();
    let checked = thread::scope(|s| {
        let checking: Vec<_> = indexed
            .chunks(indexed.len().div_ceil(workers))
            .map(|part| s.spawn(|| part.iter().map(|&(i, c)| ((i, c), holon_author::agent::plan(&flow_text("probe", base, &host, &[(i, c)])).map(|_| ()))).collect::<Vec<_>>()))
            .collect();
        checking.into_iter().flat_map(|c| c.join().unwrap()).collect::<Vec<_>>()
    });
    let fault = |e: &String| e.contains("cycle") || e.contains("this statement makes none");
    let known = |c: &Case| KNOWN_FAULTS.iter().any(|(shape, mark, _)| c.shape == *shape && c.mark == *mark);
    let faults: Vec<String> = checked.iter().filter_map(|((_, c), r)| r.as_ref().err().filter(|e| fault(e) && !known(c)).map(|e| format!("{} | {} | {} | {e}", c.shape, c.mark, c.condition))).collect();
    assert!(faults.is_empty(), "{} handlers compile to a graph that can't run:\n{}", faults.len(), faults.join("\n"));
    for (shape, mark, why) in KNOWN_FAULTS.iter().filter(|_| block == Block::Route) {
        let still = checked.iter().any(|((_, c), r)| c.shape == *shape && c.mark == *mark && r.as_ref().err().is_some_and(fault));
        assert!(still, "\"{shape}\" with {mark} compiles now: strike it from KNOWN_FAULTS ({why})");
    }
    // A handler the reader takes and the round trip refuses (the canvas can't write it back as the
    // same steps) is no refusal of the language but a gap between its two translators: each is
    // owed under a named debt, and a debt nothing is owed under any more fails, to be struck. Such a
    // handler is still run, deployed by its hash with the reader alone, so what it does is held too.
    let mut round_trip_owed: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unowed = Vec::new();
    let mut read_alone: Vec<(usize, &Case)> = Vec::new();
    for ((i, c), r) in &checked {
        let text = flow_text("probe", base, &host, &[(*i, c)]);
        if r.is_err() && holon_author::lang::ts::compile(&text).is_ok() && holon_author::lang::compile(&text).is_err() {
            read_alone.push((*i, c));
            match ROUND_TRIP_DEBTS.iter().find(|(_, owed_by, owes)| owed_by.is_none_or(|b| b == block) && owes(c)) {
                Some((debt, ..)) => *round_trip_owed.entry(debt).or_default() += 1,
                None => unowed.push(format!("{} | {} | {} | {}", c.shape, c.mark, c.condition, holon_author::lang::compile(&text).map(|_| ()).unwrap_err())),
            }
        }
    }
    // What the language refuses, by reason: a clock's or a queue's handler is refused more often
    // than a route's (no `status`), and said.
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    for (_, r) in &checked {
        if let Err(e) = r {
            let first = e.lines().next().unwrap_or("");
            *refusals.entry(first.split_once(": ").map_or(first, |(_, why)| why).chars().take(160).collect()).or_default() += 1;
        }
    }
    for (why, n) in &refusals {
        println!("  {block:?}: {n:5} refused: {why}");
    }
    let taken: Vec<(usize, &Case)> = checked.into_iter().filter(|(_, r)| r.is_ok()).map(|(c, _)| c).collect();
    assert!(!taken.is_empty(), "{block:?}: the language takes none of the grid's handlers");
    assert!(unowed.is_empty(), "{} handlers the canvas can't write back, under no debt:\n{}", unowed.len(), unowed.join("\n"));
    // A debt only this block's handlers owe is held here, at check, at either size.
    for (debt, ..) in ROUND_TRIP_DEBTS.iter().filter(|(_, owed_by, _)| *owed_by == Some(block)) {
        assert!(round_trip_owed.contains_key(debt), "\"{debt}\": the canvas writes these back now: strike it from ROUND_TRIP_DEBTS");
    }
    // The grid is no smaller than it was.
    assert!(taken.len() > 600, "{block:?}: {} handlers taken", taken.len());
    // At a size that runs none of this block's handlers, what is held is what is said at check.
    if requests.is_empty() {
        println!("{block:?}: {} handlers, {} the language takes, run on no request at this size ({size:?})", all.len(), taken.len());
        let _ = std::fs::remove_dir_all(&secrets);
        return;
    }
    // A flow of forty handlers at a time, each flow's runs one after another (the log is read by
    // what it gained), the flows side by side.
    #[derive(Default)]
    struct Tally<'a> {
        runs: usize,
        as_javascript: usize,
        /// Runs owed, each once, however many debts it is owed under.
        owed_runs: usize,
        wrong: Vec<String>,
        owed: BTreeMap<&'static str, usize>,
        /// Where a flow's own rule shows: by the condition that had nothing to go on.
        shows: BTreeMap<&'a str, usize>,
        /// Each kind of finding seen, and the first run it was seen in.
        seen: BTreeMap<Kind, String>,
        /// The kinds seen in runs on the small set's requests.
        seen_small: BTreeSet<Kind>,
    }
    impl Tally<'_> {
        fn saw(&mut self, kind: Kind, request: usize, at: impl FnOnce() -> String) {
            if SMALL_REQUESTS.contains(&request) {
                self.seen_small.insert(kind);
            }
            self.seen.entry(kind).or_insert_with(at);
        }
    }
    /// A flow of the grid: its number, its handlers, and whether it is deployed read alone.
    type GridFlow<'a> = (usize, &'a [(usize, &'a Case)], bool);
    let flows: Vec<GridFlow> = taken.chunks(40).map(|c| (c, false)).chain(read_alone.chunks(40).map(|c| (c, true))).enumerate().map(|(n, (c, alone))| (n, c, alone)).collect();
    let tallies: Vec<Tally> = thread::scope(|s| {
        let running: Vec<_> = flows
            .chunks(flows.len().div_ceil(workers))
            .enumerate()
            .map(|(w, part)| {
                let (state, host, requests) = (&state, &host, &requests);
                s.spawn(move || {
                    let mut t = Tally::default();
                    for (n, chunk, alone) in part {
                        // A queue's, a mail's or a feed's flow listens to its worker's sources.
                        // (A source is imported under the name it is bound by.)
                        let port = if block == Block::Socket { free_port() } else { base + 1 + *n as u16 };
                        let mut text = flow_text(&format!("grid-{n}"), port, host, chunk);
                        if let Some((name, kind)) = block.source() {
                            text = text
                                .replace(&format!("import {name}"), &format!("import w{w}{name}"))
                                .replace(&format!("from \"{kind}:{name}"), &format!("from \"{kind}:w{w}{name}"))
                                .replace(&format!("on.{kind}({name}"), &format!("on.{kind}(w{w}{name}"));
                        }
                        let slug = if *alone {
                            deploy_read_alone(state, &text, host)
                        } else if text.contains("host:") {
                            deploy_text(state, &text, host)
                        } else {
                            deploy_plain_text(state, &text)
                        };
                        if block == Block::Socket {
                            sync_listeners(state);
                        }
                        let javascript = javascript_does(&chunk.iter().map(|(_, c)| *c).collect::<Vec<_>>(), requests);
                        let mut seen = None;
                        for (j, ((i, case), said)) in chunk.iter().zip(&javascript).enumerate() {
                            for (&r, (written, read)) in requests.iter().zip(said) {
                                let request = REQUESTS[r];
                                let at = || format!("{}{} | {} | {} | request {r}, {request}", if *alone { "[read alone] " } else { "" }, case.shape, case.mark, case.condition);
                                let did = flow_does(state, &slug, block, (*i, j, w), request, &mut seen);
                                t.runs += 1;
                                assert!(!read.threw, "as a flow reads it, nothing throws: {} | {} | {request}", case.shape, case.condition);
                                // Each run is counted under one heading: as JavaScript (the flow
                                // does what the text and its reading both do), by a flow's own
                                // rule (it does what the reading does, which isn't what the text
                                // does), or owed (it does neither). A run that does as JavaScript
                                // and not as the reading is owed: the reading is the yardstick.
                                if did == read.marks {
                                    if did == written.marks && !written.threw {
                                        t.as_javascript += 1;
                                    } else {
                                        *t.shows.entry(case.condition).or_default() += 1;
                                        t.saw(Kind::OwnRule(case.condition), r, at);
                                    }
                                    continue;
                                }
                                let (made, missed) = apart(&did, &read.marks);
                                match debt(case, request, &made, &missed) {
                                    Some(owing) => {
                                        t.owed_runs += 1;
                                        for d in owing {
                                            *t.owed.entry(d.name).or_default() += 1;
                                            t.saw(Kind::Debt(d.name), r, at);
                                        }
                                    }
                                    None => t.wrong.push(format!(
                                        "{}{} | {} | {} | {request} | it reads as {:?} | the flow did {did:?} | javascript {:?}{}",
                                        if *alone { "[read alone] " } else { "" },
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
                        // Its worker's sources are listened to by one deployment at a time; a
                        // websocket's port is let go.
                        if block.source().is_some() || block == Block::Socket {
                            assert_eq!(route(state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
                            assert_eq!(route(state, &req("DELETE", &format!("/deployments/{slug}"), vec![])).status_code, 200);
                        }
                    }
                    t
                })
            })
            .collect();
        running.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let (mut runs, mut as_javascript, mut owed_runs, mut wrong) = (0, 0, 0, Vec::new());
    let (mut owed, mut shows): (BTreeMap<&str, usize>, BTreeMap<&str, usize>) = Default::default();
    let (mut seen, mut seen_small) = (BTreeMap::<Kind, String>::new(), BTreeSet::<Kind>::new());
    for t in tallies {
        runs += t.runs;
        as_javascript += t.as_javascript;
        owed_runs += t.owed_runs;
        wrong.extend(t.wrong);
        t.owed.into_iter().for_each(|(k, n)| *owed.entry(k).or_default() += n);
        t.shows.into_iter().for_each(|(k, n)| *shows.entry(k).or_default() += n);
        t.seen.into_iter().for_each(|(k, at)| {
            seen.entry(k).or_insert(at);
        });
        seen_small.extend(t.seen_small);
    }
    let by_rule = shows.values().sum::<usize>();
    println!("{block:?}: {} handlers, {} the language takes, {runs} runs: {as_javascript} as JavaScript, {by_rule} by a flow's own rule, {owed_runs} owed, {} wrong", all.len(), taken.len(), wrong.len());
    assert_eq!(as_javascript + by_rule + owed_runs + wrong.len(), runs, "each run is counted under one heading");
    for (condition, n) in &shows {
        println!("  {n:4} runs where `{condition}` had nothing to go on");
    }
    for (debt, n) in &round_trip_owed {
        println!("  {n:4} handlers the canvas can't write back, under \"{debt}\"");
    }
    // A run owed under two debts (a failed try that keeps both a function's write and a start)
    // shows under each: these add up to more than the runs owed.
    for (owing, n) in &owed {
        println!("  {n:4} runs owed under \"{owing}\"");
    }
    for (kind, at) in &seen {
        println!("  kind: {kind} | {} | first at {at}", if seen_small.contains(kind) { "on the small set's requests" } else { "not on the small set's requests" });
    }
    let _ = std::fs::remove_dir_all(&secrets);
    assert!(wrong.is_empty(), "{} runs don't do what their text says:
{}", wrong.len(), wrong.join("
"));
    // The grid is no smaller than it was, and most of it is plain JavaScript.
    assert!(runs > 5_000, "{block:?}: {runs} runs");
    // That most runs do as JavaScript is a claim about the whole grid's mix, held by the full set:
    // the small set keeps the requests that show the most differences, so its share is lower
    // (45% on 2026-11-23, against 61% in full). It is said, not held, there.
    println!("  {as_javascript} of {runs} runs ({}%) do as JavaScript does", as_javascript * 100 / runs);
    if size == GridSize::Full {
        assert!(as_javascript * 2 > runs, "{as_javascript} of {runs} runs do as JavaScript does");
    }
    // A debt is struck when no route owes it: a clock's or a queue's grid may owe fewer.
    if block != Block::Route {
        return;
    }
    for (debt, ..) in ROUND_TRIP_DEBTS.iter().filter(|(_, owed_by, _)| owed_by.is_none()) {
        assert!(round_trip_owed.contains_key(debt), "\"{debt}\": the canvas writes these back now: strike it from ROUND_TRIP_DEBTS");
    }
    // Every kind of finding shows on the small set's requests, at either size: the full set holds,
    // on its own runs, that the small set loses none, and says where a lost one showed. A clock's
    // and a queue's grid can show no kind but these (`debt` answers only `DEBTS`, a flow's own rule
    // shows under a condition of `CONDITIONS`), so the small set, which runs neither, loses none of
    // theirs either.
    let lost: Vec<String> = Kind::all()
        .filter(|kind| !seen_small.contains(kind))
        .map(|kind| match (size, seen.get(&kind), kind) {
            (GridSize::Full, Some(at), _) => format!("{kind}: shows only on requests the small set doesn't run (SMALL_REQUESTS), first at {at}: add a request that shows it"),
            (GridSize::Full, None, Kind::Debt(name)) => format!("{kind}: no longer differs anywhere: strike it from DEBTS ({})", DEBTS.iter().find(|d| d.name == name).map_or("", |d| d.says)),
            (GridSize::Full, None, Kind::OwnRule(_)) => format!("{kind}: no run leaves this condition nothing to go on: REQUESTS lost the request that did"),
            (GridSize::Small, ..) => format!("{kind}: the small set's requests don't show it: run the full set (HOLON_GRIDS=full) to see where it shows, or whether it shows at all"),
        })
        .collect();
    assert!(lost.is_empty(), "{} kinds of finding the small set doesn't show:
{}", lost.len(), lost.join("
"));
}

/// A kind of finding of the handler grid: a debt owed, or a flow's own rule shown under a
/// condition with nothing to go on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Debt(&'static str),
    OwnRule(&'static str),
}

impl Kind {
    /// Every kind the grid can show.
    fn all() -> impl Iterator<Item = Kind> {
        DEBTS.iter().map(|d| Kind::Debt(d.name)).chain(CONDITIONS.iter().map(|(c, _, _)| Kind::OwnRule(c)))
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::Debt(name) => write!(f, "debt \"{name}\""),
            Kind::OwnRule(condition) => write!(f, "own rule on `{condition}`"),
        }
    }
}
