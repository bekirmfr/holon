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
    capabilities, capabilities_of, eval, Capability, Hash, Param, Resolver, Type, Unit,
    UnitBody, Value,
};

/// The platform: the one place IO happens. A real host sends the mail / writes the file
/// / stores the value; a test host records. `holon-core` never sees this trait.
pub trait Host {
    /// Perform one settled request — the **only** impure operation in the system.
    fn perform(&mut self, capability: Capability, payload: &Value);
}

/// Deploy-mode firing: settle the graph (core — the one decider), then perform the
/// request its output carries (host — the only impurity). `eval` is *called*, never
/// re-implemented (`I5`). Performs the **output-side** capabilities (`!is_sourced` —
/// sinks *and* state writes); the sourced side (drivers, state reads) is supplied, not
/// performed.
pub fn fire(root: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver, host: &mut impl Host) {
    let Some(payload) = eval(root, inputs, resolver) else { return };
    let caps = capabilities(root, resolver);
    for cap in caps.iter() {
        if !cap.is_sourced() {
            host.perform(cap, &payload);
        }
    }
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

    /// One firing round: snapshot storage, fire with `store_read` sourced from the
    /// snapshot, then apply the `StateWrite` to storage. Reads see the round-start
    /// value; the write lands at round-end and loops back to the next round's read.
    pub fn round(&mut self, graph: &Unit, real: &impl Resolver) {
        // Read-side: inject the snapshot at every `store_read`. `read_value` is owned by
        // the resolver (it is built by value, not a borrow of `self`), so `self` stays
        // free to be the &mut Host below.
        let resolver = StateResolver { read_value: constant_unit(&self.storage), real };
        self.pending.clear();
        fire(graph, &[], &resolver, self);
        // Write-side: apply at round-end. (Other output-side effects — egress, … — would
        // be performed for real here; the counter has only the state write.)
        for (cap, payload) in std::mem::take(&mut self.pending) {
            if cap == Capability::StateWrite {
                self.storage = payload;
            }
        }
    }
}

impl Host for StateHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.pending.push((capability, payload.clone()));
    }
}

/// A resolver that injects the host's held value at every `store_read` leaf (one
/// anonymous cell). It **owns** the constant it hands out, since `resolve` returns
/// `&Unit`. Only `StateRead` leaves are substituted — sinks and `store_write` are left
/// real, so the performed manifest is intact.
struct StateResolver<'a, R: Resolver> {
    read_value: Unit,
    real: &'a R,
}

impl<R: Resolver> Resolver for StateResolver<'_, R> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        let u = self.real.resolve(hash)?;
        if let UnitBody::Primitive { name } = &u.body {
            if capabilities_of(name).is_some_and(|c| c.contains(Capability::StateRead)) {
                return Some(&self.read_value);
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
            output: Some(Type::Text),
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
            output: Some(Type::Text),
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
        let wh = r.insert(prim("store_write", vec![nport("v")], Type::Number)).unwrap();
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
            output: Some(Type::Number),
        };
        let mut host = Recorder::default();
        fire(&comp, &[Some(num(42.0))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::StateWrite, num(42.0))]);
    }

    #[test]
    fn counter_remembers_across_firings() {
        // store_read → +1 → store_write. The host threads storage through each round:
        // reads see the prior write, and `eval` performed none of it.
        let mut r = MapResolver::new();
        let rd = r.insert(prim("store_read", vec![], Type::Number)).unwrap();
        let one = r.insert(number("1")).unwrap();
        let add = r.insert(prim("add", vec![nport("a"), nport("b")], Type::Number)).unwrap();
        let wr = r.insert(prim("store_write", vec![nport("v")], Type::Number)).unwrap();
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
            output: Some(Type::Number),
        };

        let mut host = StateHost::new(num(0.0));
        assert_eq!(host.storage(), &num(0.0));
        host.round(&counter, &r);
        assert_eq!(host.storage(), &num(1.0));
        host.round(&counter, &r);
        assert_eq!(host.storage(), &num(2.0));
        host.round(&counter, &r);
        assert_eq!(host.storage(), &num(3.0));
    }
}