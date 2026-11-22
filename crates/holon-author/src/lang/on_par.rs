//! The two translators kept on par: the way in (`ts.rs` and `lower.rs`, a text into steps) and
//! the way back (`print.rs`, steps into text) agree, both ways, held by standing tests.
//!
//! - Text → steps → text → steps: every text `lang::compile` takes is written back and read again
//!   as the same steps (`agree.rs`), so it holds by construction for each text read: the samples,
//!   the twelve case flows (`agree.rs`), the language's grid (holon's `tests/semantics.rs`), the
//!   manual's flows (`tests/manual.rs`) and every text of every test that compiles one.
//! - Steps → text → steps, for steps drawn on the canvas that never had a text: every leaf of the
//!   live catalog and every template, each placed alone in the smallest document that holds it,
//!   printed, the print compiled, the same unit required. A leaf or template that doesn't is on
//!   `NOT_YET`, with its reason; one that starts to go both ways fails until struck.
//! - What the way in takes and the way back can't write: `OWED`, one table of the
//!   smallest text of each such form, each refused today with its mistake; a row that starts to
//!   be taken fails until it is moved to the texts taken (`agree.rs`'s table).

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use holon_core::{genesis_units, MapResolver, UnitBody, TRIGGER};

    use crate::builder::{FlowBuilder, FlowDocument, Settings};
    use crate::composites::Names;
    use crate::flow::flow_unit;
    use crate::lang::{compile, print};
    use crate::templates::TEMPLATES;
    use crate::text::restore;
    use crate::to_hex;

    /// The leaves and templates the canvas places that the way back can't yet write as the same
    /// steps, each with why. The list only shrinks: a name here that goes both ways fails.
    const NOT_YET: &[(&str, &str)] = &[
        ("not", "`!` written of a value not known to be a Boolean reads back as a truthiness test first: other steps"),
        ("guard", "a guard on a value not known to be a Boolean is written `c ? v : undefined`, which reads back with a truthiness test: other steps"),
        ("http_stream", "a streamed request placed alone is written as an answer the reader refuses: it is only taken as `respond(sent.body, { stream: true })`"),
        ("await", "a wait drawn on the canvas has none of the hold the text adds after every wait, so its text reads back as other steps"),
        ("approval", "a wait for people drawn on the canvas has none of the hold the text adds after every wait, so its text reads back as other steps"),
    ];

    /// The flow unit `doc` deploys as, in hex.
    fn unit_of(doc: &FlowDocument) -> Result<String, String> {
        let mut store = MapResolver::new();
        restore(doc, &mut store, &Names::new());
        let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
        flow_unit(&doc.nodes, &doc.edges, &mut store, port, doc.unit_settings()).map(|h| to_hex(&h))
    }

    /// `b`'s document printed, the print compiled, and the same unit required.
    fn goes_both_ways(b: FlowBuilder) -> Result<(), String> {
        let doc = b.finish(Settings { port: "8080".into(), ..Default::default() });
        let was = unit_of(&doc).map_err(|e| format!("the canvas doesn't build: {e}"))?;
        let text = print::print(Some("f"), &doc, &mut MapResolver::new(), &Names::new(), &BTreeMap::new()).map_err(|e| format!("not written: {e}"))?;
        let back = compile(&text).map_err(|e| format!("written, doesn't read back: {e}\n{text}"))?;
        let is = unit_of(&back.doc).map_err(|e| format!("read back, doesn't build: {e}"))?;
        if is != was {
            return Err(format!("read back as other steps:\n{text}"));
        }
        Ok(())
    }

    /// How a leaf is placed in the smallest document that holds it. The plainest first (under a
    /// route, its params the catalog's, its first answer answered); each other way is taken only
    /// when the plainer one says it needs it, in the builder's or the printer's own words.
    #[derive(Clone, Copy, Debug, Default)]
    struct Placing {
        /// Under an action, for a leaf that waits ("a route answers and is done, so it doesn't
        /// wait").
        waits: bool,
        /// Given a function, for a leaf that applies one ("without a function has no TypeScript
        /// form"): a function placing the first catalog leaf of the inputs it hands.
        function: bool,
        /// Its first output answered though it is an effect, for a leaf that is a route's answer
        /// ("add a respond node").
        answers: bool,
    }

    /// What the plainer placing said that asks for the next one, if it did.
    fn next(placing: Placing, said: &str) -> Option<Placing> {
        if said.contains("so it doesn't wait") && !placing.waits {
            return Some(Placing { waits: true, ..placing });
        }
        if said.contains("without a function has no TypeScript form") && !placing.function {
            return Some(Placing { function: true, ..placing });
        }
        if said.contains("add a respond node") && !placing.answers {
            return Some(Placing { answers: true, ..placing });
        }
        None
    }

    /// `leaf` (a catalog leaf, or a template placed as one) alone in the smallest document that
    /// holds it, both ways.
    fn leaf_goes_both_ways(leaf: &str) -> Result<(), String> {
        let mut placing = Placing::default();
        loop {
            match placed(leaf, placing) {
                Err(e) => match next(placing, &e) {
                    Some(further) => placing = further,
                    None => return Err(e),
                },
                ok => return ok,
            }
        }
    }

    /// The first leaf of the catalog taking `n` inputs, with no params and one answer, that goes
    /// both ways itself, placed as a function, as the canvas makes one: its hash.
    fn function_of(b: &mut FlowBuilder, n: usize) -> Result<String, String> {
        let leaf = genesis_units()
            .iter()
            .find_map(|u| match &u.body {
                UnitBody::Primitive { name } if u.inputs.len() == n && u.outputs.len() == 1 && u.params.is_empty() && placed(name, Placing::default()).is_ok() => Some(name.clone()),
                _ => None,
            })
            .ok_or("no leaf to be a function")?;
        b.begin_holon("fn_1");
        let id = b.node(&leaf, &[], None)?;
        let data = b.data(&id).expect("placed").clone();
        let ins: Vec<String> = data.inputs.iter().map(|p| p.name.clone()).collect();
        let outs: Vec<String> = data.outputs.iter().map(|p| p.name.clone()).collect();
        Ok(b.end_holon("fn_1", &ins, &outs)?.hash)
    }

    /// `leaf` placed as `placing` says: each input wired from what its handler gives (its trigger
    /// too), its params as the catalog gives them, its first answer answered; both ways.
    fn placed(leaf: &str, placing: Placing) -> Result<(), String> {
        let mut b = FlowBuilder::new();
        let (h, values, request) = match placing.waits {
            false => (b.node("request", &[("method".into(), "POST".into()), ("path".into(), "/x".into())], None)?, "json", "request"),
            true => (b.node("action", &[("name".into(), "t".into())], None)?, "input", "input"),
        };
        let set: Vec<(String, String)> = match placing.function {
            true => vec![("fn".into(), function_of(&mut b, holon_core::function_takes(leaf))?)],
            false => vec![],
        };
        let id = b.node(leaf, &set, None)?;
        let data = b.data(&id).expect("placed").clone();
        for p in &data.inputs {
            let from = if p.name == TRIGGER { request } else { values };
            b.wire(&h, from, &id, &p.name)?;
        }
        let answer = data.outputs.iter().find(|p| placing.answers || p.ty != crate::wire::WType::Effect).map(|p| p.name.clone());
        if let Some(out) = answer {
            let respond = b.node("respond", &[], None)?;
            b.wire(&h, request, &respond, "request")?;
            b.wire(&id, &out, &respond, "body")?;
        }
        goes_both_ways(b)
    }

    /// A template alone in the smallest document that holds it, both ways. A handler (`every`,
    /// `on_file`, an action…) with one effect, a log line of the first thing it gives, tied to
    /// it; a route, `request` and `respond`, answering the request's JSON; any other template
    /// placed under a route as a leaf is.
    fn template_goes_both_ways(template: &str) -> Result<(), String> {
        let mut b = FlowBuilder::new();
        match template {
            "request" | "respond" => {
                let req = b.node("request", &[("method".into(), "POST".into()), ("path".into(), "/x".into())], None)?;
                let respond = b.node("respond", &[], None)?;
                b.wire(&req, "request", &respond, "request")?;
                b.wire(&req, "json", &respond, "body")?;
            }
            _ if crate::templates::interface(template, &crate::templates::defaults(template).unwrap_or_default()).is_some_and(|i| i.hidden_inputs || i.inputs.is_empty()) => {
                let h = b.node(template, &[], None)?;
                let given = b.data(&h).expect("placed").outputs.first().map(|p| p.name.clone()).ok_or("gives nothing")?;
                let log = b.node("log", &[], None)?;
                for p in b.data(&log).expect("placed").inputs.clone() {
                    b.wire(&h, &given, &log, &p.name)?;
                }
            }
            _ => return leaf_goes_both_ways(template),
        }
        goes_both_ways(b)
    }

    /// Every leaf in the live catalog (a primitive of `genesis_units`).
    fn catalog() -> Vec<String> {
        genesis_units()
            .iter()
            .filter_map(|u| match &u.body {
                UnitBody::Primitive { name } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// **Guards:** what is drawn on the canvas goes both ways: every leaf of the live catalog and
    /// every template, placed alone in the smallest document that holds it, is printed as a text
    /// that compiles to the same unit, or is on [`NOT_YET`] with why. **Why:** the canvas and the
    /// text are two ways to write one flow, translated by hand each way (`ts.rs` and `lower.rs`
    /// in, `print.rs` back); a leaf or a form one of them learns and the other doesn't is a flow
    /// someone can draw and not read, or read and not draw. Driven by the catalog, a new leaf
    /// fails here until both translators take it or it is listed. **If it fails:** a leaf or
    /// template doesn't go both ways: teach the translator that lacks it, or list it with its
    /// reason; one listed now goes both ways: strike it from the list.
    #[test]
    fn every_leaf_and_template_goes_both_ways() {
        let all: Vec<String> = catalog().into_iter().chain(TEMPLATES.iter().map(|t| t.to_string())).collect();
        let mut wrong = Vec::new();
        for name in &all {
            let went = match TEMPLATES.contains(&name.as_str()) {
                true => template_goes_both_ways(name),
                false => leaf_goes_both_ways(name),
            };
            match (went, NOT_YET.iter().find(|(n, _)| n == name)) {
                (Ok(()), Some(_)) => wrong.push(format!("{name}: goes both ways now: strike it from NOT_YET")),
                (Err(e), None) => wrong.push(format!("{name}: doesn't go both ways, and isn't on NOT_YET: {e}")),
                _ => {}
            }
        }
        for (name, _) in NOT_YET {
            if !all.iter().any(|n| n == name) {
                wrong.push(format!("{name}: on NOT_YET, and no leaf or template of the catalog"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
    }

    /// The forms the way in takes and the way back can't write, each by its smallest text and
    /// the mistake `lang::compile` refuses it with (its line, and its words): what is owed. A
    /// form the printer learns is taken; its row then fails, to be moved to the texts taken.
    pub(super) const OWED: &[(&str, &str, u32, &str)] = &[
        (
            "a function of the flow starting another flow's action",
            "import { action, flow, route } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nexport const ping = action((x) => x)\n\nfunction ask(x) {\n  ping(x)\n  return 1\n}\n\nroute.post(\"/\", (req) => ask(req.json))\n",
            7,
            "the function \"ask\" starts another flow's action: a function only computes, so start it in the route or action that calls \"ask\"",
        ),
        (
            "a write in a list method's function",
            "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"f\", { port: 8080 })\n\nfunction keep(x) {\n  ++state.n\n  return `${x}!`\n}\n\nroute.post(\"/\", (req) => req.json.map((x) => keep(x)))\n",
            11,
            "the function given to map has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: move it out of the function, into the route or action itself",
        ),
        (
            "a request for each item of a list",
            "import { flow, route } from \"holon\"\nimport api from \"host:api.example.com\"\n\nflow(\"f\", { port: 8080 })\n\nroute.post(\"/\", (req) => req.json.map((x) => api.get(`/a/${x}`).status))\n",
            6,
            "the function given to map has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: move it out of the function, into the route or action itself",
        ),
        (
            "a request on one side of a ?: in a function",
            "import { flow, route } from \"holon\"\nimport api from \"host:api.example.com\"\n\nflow(\"f\", { port: 8080 })\n\nasync function ask(x) {\n  return x === \"a\" ? (await api.post(\"/in\", { body: x })).status : 0\n}\n\nroute.post(\"/\", async (req) => await ask(req.json))\n",
            6,
            "the function \"ask\" has a write, a request or a start (its own, or one in a function it calls) that the place calling it can't hold back: move it into the route or action that calls \"ask\"",
        ),
        (
            "&& around an awaited function of the flow",
            "import { flow, route } from \"holon\"\nimport api from \"host:api.example.com\"\n\nflow(\"f\", { port: 8080 })\n\nasync function ask(x) {\n  const sent = await api.post(\"/in\", { body: x })\n  return sent.status\n}\n\nroute.post(\"/\", async (req) => {\n  const got = req.json.c && await ask(\"a\")\n  return \"end\"\n})\n",
            4,
            "written back from its steps, this text reads as another flow, so the canvas would show something other than what runs: write it another way (and please report it: Holon's two readings of a text should agree)",
        ),
        (
            "|| around an awaited function of the flow",
            "import { flow, route } from \"holon\"\nimport api from \"host:api.example.com\"\n\nflow(\"f\", { port: 8080 })\n\nasync function ask(x) {\n  const sent = await api.post(\"/in\", { body: x })\n  return sent.status\n}\n\nroute.post(\"/\", async (req) => {\n  const got = req.json.c || await ask(\"a\")\n  return \"end\"\n})\n",
            4,
            "written back from its steps, this text reads as another flow, so the canvas would show something other than what runs: write it another way (and please report it: Holon's two readings of a text should agree)",
        ),
        (
            "an action awaiting its own flow's action",
            "import { action, flow } from \"holon\"\nimport { go } from \"flow:me\"\n\nflow(\"me\")\n\nexport const go = action(async (input) => {\n  const r = await go(input)\n  return r\n})\n",
            4,
            "this flow's steps can't be written back as text, so the canvas couldn't show it: await go(…) would wait for this flow's own action, which a flow can't yet: start it, go(x)",
        ),
    ];

    /// A handler of `n` early returns in a row: the text form owed for long handlers (the bound
    /// of each nesting form is `nesting.rs`'s `WRITTEN_BACK`).
    fn early_returns(n: usize) -> String {
        let rows: String = (1..=n).map(|k| format!("  if (req.json.a === {k}) {{\n    return {k}\n  }}\n")).collect();
        format!("import {{ flow, route }} from \"holon\"\n\nflow(\"f\", {{ port: 8080 }})\n\nroute.post(\"/\", (req) => {{\n{rows}  return 0\n}})\n")
    }

    /// **Guards:** what the way in takes and the way back can't write is one visible list: each
    /// row is read by the reader alone and refused by `lang::compile`, at its line, in its words;
    /// a handler of early returns is taken to 24 and refused from 25. **Why:** the owner took
    /// these refusals (2026-11-22) over teaching the printer first; this is what is owed. **If it
    /// fails:** a row is taken now: move its text to the texts taken (`agree.rs`'s table, or a
    /// sample) and strike it here; a row's words or line changed: say them here as they are.
    #[test]
    fn what_the_way_back_owes_is_refused_in_plain_words() {
        let mut wrong = Vec::new();
        for (what, text, line, said) in OWED {
            if let Err(e) = crate::lang::ts::compile(text) {
                wrong.push(format!("{what}: the reader alone refuses it: {e}"));
                continue;
            }
            match compile(text) {
                Ok(_) => wrong.push(format!("{what}: taken now: move it to the texts taken, and strike it here")),
                Err(e) if (e.span.start.line, e.message.as_str()) != (*line, *said) => wrong.push(format!("{what}: refused at {}: {}", e.span.start.line, e.message)),
                Err(_) => {}
            }
        }
        let long = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(|| (compile(&early_returns(24)).map(|_| ()).map_err(|e| e.message), compile(&early_returns(25)).map(|_| ()).map_err(|e| e.message)))
            .unwrap()
            .join()
            .unwrap();
        if long != (Ok(()), Err(crate::lang::agree::TOO_DEEP.to_string())) {
            wrong.push(format!("early returns, 24 and 25 in a row: {long:?}"));
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
