//! # holon-host — the imperative shell (deploy-mode firing + state)
//!
//! The first and only impurity. `holon-core` settles a graph to a *request* (Fork B: a
//! sink purely constructs its payload, so the request is ordinary output data); this
//! crate **performs** it. The crate dependency `holon-host → holon-core` (never the
//! reverse) *is* the purity boundary — `holon-core` carries no IO, async, or network
//! dependency, ever. The only impure function in the system is `Host::perform`.
//!
//! Deploy firing is **eager**: `eval` returns the settled value before anything is
//! performed, so no sink ever acts on a mid-propagation transient. For an acyclic graph
//! the fixed point is one `eval` pass, so each effect is performed once with its
//! committed payload.
//!
//! [`StateHost`] adds the one fenced state cell (`P2.6.1`): its storage is the `Model`;
//! a `store_read` sources it, a `store_write` (a Fork-B sink, capability `StateWrite`)
//! commits to it. The feedback that looked like a graph cycle is broken by storage
//! *between rounds* — no `validate` change, no `eval` change.

use holon_core::{
    capabilities_of, eval, Capability, Hash, Param, Resolver, Type, Unit, UnitBody, Value,
};

/// The platform: the one place IO happens. A real host sends the mail / writes the file
/// / stores the value; a test host records. `holon-core` never sees this trait.
pub trait Host {
    /// Perform one settled request — the **only** impure operation in the system.
    fn perform(&mut self, capability: Capability, payload: &Value);
}

/// Deploy-mode firing: settle the graph (core — the one decider), then perform the
/// effect(s) its output *carries* (host — the only impurity). `eval` is *called*, never
/// re-implemented (`I5`). Under representation **B** the output is a self-describing
/// `Effect { sink, payload }`, so performance is driven by the **output** (what actually
/// reached the boundary), not the manifest — the manifest stays the trust disclosure.
pub fn fire(root: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver, host: &mut impl Host) {
    let Some(output) = eval(root, inputs, resolver) else { return };
    perform_effects(&output, host);
}

/// Perform the effect(s) a settled output carries. A single `Effect`, or a `List` of them
/// (and nested lists) — the **computed fan-out**: `map(email, xs)` yields a `List[Effect]`,
/// performed in order. Pure data carries nothing. The capability is *derived* from the sink's
/// name (`capabilities_of`), so the `Host` trait is unchanged and a graph can only perform a
/// capability it structurally declares — the effect was constructed by a leaf that has it.
fn perform_effects(value: &Value, host: &mut impl Host) {
    match value {
        Value::Effect { sink, payload } => {
            if let Some(caps) = capabilities_of(sink) {
                for cap in caps.iter() {
                    if !cap.is_sourced() {
                        host.perform(cap, &**payload);
                    }
                }
            }
        }
        Value::List(items) => {
            for item in items {
                perform_effects(item, host); // flatten lists of effects (and nested lists)
            }
        }
        _ => {} // pure data at the root carries nothing to perform
    }
}

// ── Drive: source the world (and state) at sourced leaves ──────────────────────────

/// Fire a graph with its **sourced** inputs supplied: each `(capability, value)` injects
/// `value` at every leaf carrying that sourced capability — `Clock`, ingress, `StateRead`, …
/// This is the one entry every mode funnels through; a leaf with no pure `compute` (a driver,
/// a state read) gets its value here, or — if unsupplied — evaluates to nothing (deploy's
/// honest asymmetry with design mode, which substitutes a default).
pub fn fire_sourced(
    root: &Unit,
    sourced: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
) {
    let resolver = SourcedResolver::new(sourced, real);
    fire(root, &[], &resolver, host);
}

// ── State: one anonymous cell threaded across rounds (P2.6.1) ──────────────────────

/// A host that threads a single anonymous state cell across firing rounds. Storage is
/// the `Model`: a `store_read` sources the round-start snapshot, a `store_write` commits
/// the new value, which the round applies at the end — Elm's `update(old) → (new,
/// cmds)`, atomically.
pub struct StateHost {
    storage: Value,
    pending: Vec<(Capability, Value)>,
}

impl StateHost {
    pub fn new(initial: Value) -> Self {
        StateHost { storage: initial, pending: Vec::new() }
    }

    pub fn storage(&self) -> &Value {
        &self.storage
    }

    /// One firing round: source `StateRead` from storage (internal, threaded) and the given
    /// `drivers` from the world (external, this event's inputs), fire, then apply the
    /// `StateWrite` to storage. Reads see the round-start values; the write lands at round-end
    /// and loops back. With `drivers` empty this is the pure state loop.
    pub fn round(&mut self, graph: &Unit, drivers: &[(Capability, Value)], real: &impl Resolver) {
        // Sourced inputs: storage for `StateRead` (threaded), the world for the drivers. The
        // resolver injects both uniformly; only the internal one (`StateWrite`) is committed
        // back below — drivers have no write-back.
        let mut sourced = vec![(Capability::StateRead, self.storage.clone())];
        sourced.extend_from_slice(drivers);
        self.pending.clear();
        fire_sourced(graph, &sourced, real, self);
        for (cap, payload) in std::mem::take(&mut self.pending) {
            if cap == Capability::StateWrite {
                self.storage = payload; // the Model-out (loops back to next round's read)
            }
        }
    }
}

impl Host for StateHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.pending.push((capability, payload.clone()));
    }
}

/// A resolver that injects a host-supplied constant at every **sourced** leaf — `StateRead`,
/// `Clock`, ingress, … — keyed by capability (one value per kind: the anonymous cell, the
/// anonymous clock; keyed/multiple resources defer with the capability model). It **owns** the
/// constants it hands out, since `resolve` returns `&Unit`. Performed leaves (sinks,
/// `store_write`) are never substituted, so the effect manifest is intact.
struct SourcedResolver<'a, R: Resolver> {
    sourced: Vec<(Capability, Unit)>,
    real: &'a R,
}

impl<'a, R: Resolver> SourcedResolver<'a, R> {
    fn new(sourced: &[(Capability, Value)], real: &'a R) -> Self {
        let sourced = sourced.iter().map(|(cap, value)| (*cap, constant_unit(value))).collect();
        SourcedResolver { sourced, real }
    }
}

impl<R: Resolver> Resolver for SourcedResolver<'_, R> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        let u = self.real.resolve(hash)?;
        if let UnitBody::Primitive { name } = &u.body {
            if let Some(caps) = capabilities_of(name) {
                // Inject the first sourced capability we hold a value for (one per leaf in the
                // minimal set: timer→Clock, store_read→StateRead).
                for (cap, constant) in &self.sourced {
                    if caps.contains(*cap) {
                        return Some(constant);
                    }
                }
            }
        }
        Some(u)
    }
}

/// A pure leaf that evaluates to `value` — used to inject the held state at the
/// `store_read` boundary. Scalar; non-scalar state is deferred (a standalone unit can't
/// carry list items without children-by-hash, and the minimal cell is scalar).
fn constant_unit(value: &Value) -> Unit {
    match value {
        Value::Number(n) => scalar("number", n.to_string(), Type::Number),
        Value::Boolean(b) => scalar("boolean", b.to_string(), Type::Boolean),
        Value::Text(s) => scalar("text", s.clone(), Type::Text),
        Value::List(_) => Unit {
            body: UnitBody::Primitive { name: "list".into() },
            params: vec![],
            inputs: vec![],
            output: Some(Type::List),
        },
        // A state cell never holds an effect: a write commits the *payload* (`&**payload`),
        // never the request itself. If this fires, the round stored an `Effect` by mistake —
        // a bug to fix, not a value to fabricate.
        Value::Effect { .. } => unreachable!("a state cell never holds an effect"),
    }
}

fn scalar(name: &str, value: String, ty: Type) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: name.into() },
        params: vec![Param::frozen("value", value)],
        inputs: vec![],
        output: Some(ty),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{Edge, MapResolver, Port, BOUNDARY};

    /// A recording host: captures what *would* be performed, so the loop is testable
    /// without touching the world.
    #[derive(Default)]
    struct Recorder {
        performed: Vec<(Capability, Value)>,
    }
    impl Host for Recorder {
        fn perform(&mut self, capability: Capability, payload: &Value) {
            self.performed.push((capability, payload.clone()));
        }
    }

    fn email() -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "email".into() },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            output: Some(Type::Effect),
        }
    }
    fn nport(name: &str) -> Port {
        Port { name: name.into(), ty: Type::Number }
    }
    fn prim(name: &str, inputs: Vec<Port>, output: Type) -> Unit {
        Unit { body: UnitBody::Primitive { name: name.into() }, params: vec![], inputs, output: Some(output) }
    }
    fn number(n: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "number".into() },
            params: vec![Param::frozen("value", n)],
            inputs: vec![],
            output: Some(Type::Number),
        }
    }
    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn num(n: f64) -> Value {
        Value::Number(n)
    }

    #[test]
    fn bare_sink_is_performed_once_with_settled_payload() {
        let r = MapResolver::new();
        let mut host = Recorder::default();
        fire(&email(), &[Some(text("hi"))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::NetworkEgress, text("hi"))]);
    }

    #[test]
    fn sink_in_a_composite_is_performed() {
        let mut r = MapResolver::new();
        let eh = r.insert(email()).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![eh],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            output: Some(Type::Effect),
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(text("ship it"))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::NetworkEgress, text("ship it"))]);
    }

    #[test]
    fn pure_graph_performs_nothing() {
        let r = MapResolver::new();
        let constant = number("5");
        let mut host = Recorder::default();
        fire(&constant, &[], &r, &mut host);
        assert!(host.performed.is_empty());
    }

    #[test]
    fn state_write_is_performed() {
        // A store_write is now an output-side effect (`!is_sourced`): fire performs it.
        let mut r = MapResolver::new();
        let wh = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![wh],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![nport("v")],
            output: Some(Type::Effect),
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(num(42.0))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::StateWrite, num(42.0))]);
    }

    /// store_read → +1 → store_write: the canonical one-cell counter. Shared by the state and
    /// fork tests.
    fn counter_graph() -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let rd = r.insert(prim("store_read", vec![], Type::Number)).unwrap();
        let one = r.insert(number("1")).unwrap();
        let add = r.insert(prim("add", vec![nport("a"), nport("b")], Type::Number)).unwrap();
        let wr = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let counter = Unit {
            body: UnitBody::Composite {
                children: vec![rd, one, add, wr],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 2, to_port: 0 }, // read → add.a
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 1 }, // one  → add.b
                    Edge { from_child: 2, from_port: 0, to_child: 3, to_port: 0 }, // add  → write.v
                    Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Effect),
        };
        (r, counter)
    }

    #[test]
    fn counter_remembers_across_firings() {
        // store_read → +1 → store_write. The host threads storage through each round:
        // reads see the prior write, and `eval` performed none of it.
        let (r, counter) = counter_graph();
        let mut host = StateHost::new(num(0.0));
        assert_eq!(host.storage(), &num(0.0));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage(), &num(1.0));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage(), &num(2.0));
        host.round(&counter, &[], &r);
        assert_eq!(host.storage(), &num(3.0));
    }

    // ── slice 2: computed fan-out ──────────────────────────────────────────────
    fn hex(h: &Hash) -> String {
        h.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn map_fans_out_one_email_per_element() {
        // map(email, [a,b,c]) ⇒ List[Effect{email,a}, …] ⇒ three sends, in order. The
        // one-effect ceiling is lifted: a single graph performs many effects.
        let mut r = MapResolver::new();
        let eh = r.insert(email()).unwrap();
        let mapper = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&eh))],
            inputs: vec![Port { name: "xs".into(), ty: Type::List }],
            output: Some(Type::List),
        };
        let xs = Value::List(vec![text("a"), text("b"), text("c")]);
        let mut host = Recorder::default();
        fire(&mapper, &[Some(xs)], &r, &mut host);
        assert_eq!(
            host.performed,
            vec![
                (Capability::NetworkEgress, text("a")),
                (Capability::NetworkEgress, text("b")),
                (Capability::NetworkEgress, text("c")),
            ]
        );
    }

    #[test]
    fn heterogeneous_effects_fire_in_one_round() {
        // One output carrying two *different* effects — a state write AND a send — both fire,
        // with no multi-output composite: the self-describing List[Effect] is enough.
        let mut r = MapResolver::new();
        let sw = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let em = r.insert(email()).unwrap();
        let lst = r
            .insert(prim(
                "list",
                vec![
                    Port { name: "x".into(), ty: Type::Effect },
                    Port { name: "y".into(), ty: Type::Effect },
                ],
                Type::List,
            ))
            .unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![sw, em, lst],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, // v   → store_write
                    Edge { from_child: BOUNDARY, from_port: 1, to_child: 1, to_port: 0 }, // msg → email
                    Edge { from_child: 0, from_port: 0, to_child: 2, to_port: 0 },        // write → list.x
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 1 },        // email → list.y
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // list → out
                ],
            },
            params: vec![],
            inputs: vec![nport("v"), Port { name: "msg".into(), ty: Type::Text }],
            output: Some(Type::List),
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(num(7.0)), Some(text("hi"))], &r, &mut host);
        assert_eq!(
            host.performed,
            vec![
                (Capability::StateWrite, num(7.0)),
                (Capability::NetworkEgress, text("hi")),
            ]
        );
    }

    // ── drive: source the world ────────────────────────────────────────────────
    /// timer (Clock) → store_write: read the clock, commit it. A graph whose driver leaf
    /// (`timer`) has no pure compute — it must be sourced.
    fn timer_to_store() -> (MapResolver, Unit) {
        let mut r = MapResolver::new();
        let th = r.insert(prim("timer", vec![], Type::Number)).unwrap();
        let wh = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let graph = Unit {
            body: UnitBody::Composite {
                children: vec![th, wh],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // timer → write.v
                    Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Effect),
        };
        (r, graph)
    }

    #[test]
    fn round_sources_a_driver() {
        // A driver (Clock) is sourced from the round's supply — the world flows in, through the
        // graph, and here lands in storage. One round threads state AND injects the world.
        let (r, graph) = timer_to_store();
        let mut host = StateHost::new(num(0.0));
        host.round(&graph, &[(Capability::Clock, num(99.0))], &r);
        assert_eq!(host.storage(), &num(99.0));
    }

    #[test]
    fn an_undriven_driver_yields_nothing() {
        // Deploy's deliberate asymmetry with design mode: an undriven driver has no compute, so
        // the graph evaluates to None and nothing is performed — no silent default.
        let (r, graph) = timer_to_store();
        let mut host = Recorder::default();
        fire_sourced(&graph, &[], &r, &mut host); // no Clock supplied
        assert!(host.performed.is_empty());
    }

    #[test]
    fn forking_a_cells_state_is_copy_on_write() {
        // P2.6.3, the only sub-part with observable behavior. Fork = a new host over a *clone* of
        // the current state; the two then diverge independently. Copy-on-write falls out of
        // storage living in the host (P2.6.1), not the unit — no new mechanism. Identity is
        // shared (both run the same `&counter`, one graph); state is per-host.
        let (r, counter) = counter_graph();
        let mut original = StateHost::new(num(5.0));
        original.round(&counter, &[], &r); // original: 6

        let mut fork = StateHost::new(original.storage().clone()); // CoW: copy current state (6)
        original.round(&counter, &[], &r); // original: 7
        original.round(&counter, &[], &r); // original: 8
        fork.round(&counter, &[], &r); // fork: 7 (from its own 6, untouched by original)

        assert_eq!(original.storage(), &num(8.0));
        assert_eq!(fork.storage(), &num(7.0)); // diverged — independent runtime state
    }

    #[test]
    fn saturating_counter_is_one_atomic_conditional_update() {
        // P2.6.2 by dissolution. A conditional read-modify-write — write N+1 only while N < cap,
        // else hold — *composes* from store_read → compare → select → store_write, and the round
        // makes it atomic: the value written is computed from the value read with no gap, because
        // a round is snapshot → fire → apply and rounds are serialized (`&mut self`). The round
        // IS the transaction; no special primitive. Caps at 2: 0 → 1 → 2 → 2 → 2.
        let mut r = MapResolver::new();
        let rd = r.insert(prim("store_read", vec![], Type::Number)).unwrap();
        let one = r.insert(number("1")).unwrap();
        let cap = r.insert(number("2")).unwrap();
        let add = r.insert(prim("add", vec![nport("a"), nport("b")], Type::Number)).unwrap();
        let cmp = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "compare".into() },
                params: vec![Param::frozen("op", "<")],
                inputs: vec![nport("a"), nport("b")],
                output: Some(Type::Boolean),
            })
            .unwrap();
        let sel = r
            .insert(prim(
                "select",
                vec![Port { name: "cond".into(), ty: Type::Boolean }, nport("then"), nport("else")],
                Type::Number,
            ))
            .unwrap();
        let wr = r.insert(prim("store_write", vec![nport("v")], Type::Effect)).unwrap();
        let saturating = Unit {
            body: UnitBody::Composite {
                children: vec![rd, one, cap, add, cmp, sel, wr],
                edges: vec![
                    Edge { from_child: 0, from_port: 0, to_child: 3, to_port: 0 }, // N   → add.a
                    Edge { from_child: 1, from_port: 0, to_child: 3, to_port: 1 }, // 1   → add.b
                    Edge { from_child: 0, from_port: 0, to_child: 4, to_port: 0 }, // N   → cmp.a
                    Edge { from_child: 2, from_port: 0, to_child: 4, to_port: 1 }, // cap → cmp.b
                    Edge { from_child: 4, from_port: 0, to_child: 5, to_port: 0 }, // N<cap → sel.cond
                    Edge { from_child: 3, from_port: 0, to_child: 5, to_port: 1 }, // N+1 → sel.then
                    Edge { from_child: 0, from_port: 0, to_child: 5, to_port: 2 }, // N   → sel.else
                    Edge { from_child: 5, from_port: 0, to_child: 6, to_port: 0 }, // chosen → write.v
                    Edge { from_child: 6, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // write → out
                ],
            },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Effect),
        };

        let mut host = StateHost::new(num(0.0));
        let mut seen = vec![host.storage().clone()];
        for _ in 0..4 {
            host.round(&saturating, &[], &r);
            seen.push(host.storage().clone());
        }
        assert_eq!(seen, vec![num(0.0), num(1.0), num(2.0), num(2.0), num(2.0)]);
    }

    #[test]
    fn concurrent_requests_keep_their_tokens_no_crossing() {
        // P2.5.1: two requests, each `[token, data]`, processed by `map(respond, ·)`. Each reply
        // carries *its own* token because each flowed down its own pure path — non-crossing is
        // purity's theorem, not an engine feature. The token is just the boundary label the host
        // would route the reply on (real routing → platform host). Token 1 stays with "alpha",
        // token 2 with "beta"; neither crosses.
        let mut r = MapResolver::new();
        let respond = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "respond".into() },
                params: vec![],
                inputs: vec![Port { name: "req".into(), ty: Type::List }],
                output: Some(Type::Effect),
            })
            .unwrap();
        let mapper = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&respond))],
            inputs: vec![Port { name: "requests".into(), ty: Type::List }],
            output: Some(Type::List),
        };
        let batch = Value::List(vec![
            Value::List(vec![num(1.0), text("alpha")]), // [token=1, data="alpha"]
            Value::List(vec![num(2.0), text("beta")]),  // [token=2, data="beta"]
        ]);
        let mut host = Recorder::default();
        fire(&mapper, &[Some(batch)], &r, &mut host);
        assert_eq!(
            host.performed,
            vec![
                (Capability::NetworkEgress, Value::List(vec![num(1.0), text("alpha")])),
                (Capability::NetworkEgress, Value::List(vec![num(2.0), text("beta")])),
            ]
        );
    }

    /// Models the external system's idempotency: it remembers the keys it has performed and
    /// ignores a repeat. Dedup is on the key (`payload[0]` of a `[key, body]` pair), not the body.
    #[derive(Default)]
    struct DedupeHost {
        seen: Vec<Value>,
        performed: Vec<(Capability, Value)>,
    }
    impl Host for DedupeHost {
        fn perform(&mut self, capability: Capability, payload: &Value) {
            let key = match payload {
                Value::List(items) => items.first().cloned(),
                _ => None,
            };
            if let Some(k) = key {
                if self.seen.contains(&k) {
                    return; // already performed this key — the boundary ignores the double-fire
                }
                self.seen.push(k);
            }
            self.performed.push((capability, payload.clone()));
        }
    }

    #[test]
    fn an_idempotency_key_dedupes_a_refire() {
        // P2.5.2: the same intent fired twice reconstructs the *same* key — a pure graph is
        // deterministic (P1.4.4), so re-evaluation rebuilds an identical effect, key included.
        // The external system (modeled by DedupeHost) recognizes the repeated key and performs
        // once. Purity is both why the re-fire reproduces the effect AND why dedup works (the key
        // is stable). The engine carries the key; the user derives it from intent (here, the id).
        let mut r = MapResolver::new();
        let welcome = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "text".into() },
                params: vec![Param::frozen("value", "welcome")],
                inputs: vec![],
                output: Some(Type::Text),
            })
            .unwrap();
        let pair = r
            .insert(prim(
                "list",
                vec![
                    Port { name: "key".into(), ty: Type::Text },
                    Port { name: "body".into(), ty: Type::Text },
                ],
                Type::List,
            ))
            .unwrap();
        let em = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "email".into() },
                params: vec![],
                inputs: vec![Port { name: "msg".into(), ty: Type::List }],
                output: Some(Type::Effect),
            })
            .unwrap();
        let graph = Unit {
            body: UnitBody::Composite {
                children: vec![welcome, pair, em],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 1, to_port: 0 }, // id → key (intent-derived)
                    Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 1 },        // "welcome" → body
                    Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 },        // [key,body] → email
                    Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // email → out
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "id".into(), ty: Type::Text }],
            output: Some(Type::Effect),
        };

        let order = Value::Text("order-42".into());
        let mut host = DedupeHost::default();
        fire(&graph, &[Some(order.clone())], &r, &mut host); // first fire — performed
        fire(&graph, &[Some(order)], &r, &mut host); // re-fire, same intent — same key, deduped
        assert_eq!(
            host.performed,
            vec![(Capability::NetworkEgress, Value::List(vec![text("order-42"), text("welcome")]))]
        );
    }
}