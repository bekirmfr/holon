//! # holon-host — the imperative shell (deploy-mode firing)
//!
//! The first and only impurity. `holon-core` settles a graph to a *request* (Fork B: a
//! sink purely constructs its payload, so the request is ordinary output data); this
//! crate **performs** it. The crate dependency `holon-host → holon-core` (never the
//! reverse) *is* the purity boundary — `holon-core` carries no IO, async, or network
//! dependency, ever.
//!
//! Deploy firing is **eager**: `eval` returns the settled value before anything is
//! performed, so no sink ever acts on a mid-propagation transient — settle-then-perform
//! is ordering, never a debounce. For an acyclic graph the fixed point is one `eval`
//! pass, so each effect is performed once with its committed payload.
//!
//! This is the minimal crossing: a single terminal sink, inputs handed in directly.
//! Driver sourcing (`drive`), computed effect fan-out (`Value::Effect` / `List`), and
//! state (the `Model` half of the round) are the next seams.

use holon_core::{capabilities, eval, Capability, Kind, Resolver, Unit, Value};

/// The platform: the one place IO happens. A real host sends the mail / writes the
/// file / makes the request; a test host records. `holon-core` never sees this trait.
pub trait Host {
    /// Perform one settled request — the **only** impure operation in the system.
    fn perform(&mut self, capability: Capability, payload: &Value);
}

/// Deploy-mode firing: settle the graph (core — the one decider), then perform the
/// request its output carries (host — the only impurity). `eval` is *called*, never
/// re-implemented, so the editor's evaluator and the host's stay bit-identical (`I5`).
///
/// Minimal shape (representation **A**): a single terminal sink — the settled output
/// *is* the payload, and the manifest names the egress. Computed fan-out
/// (`List[Effect]`) is the **B** upgrade; it changes the *output shape*, not this loop.
/// (Under A a multi-sink graph would perform one payload for each sink, which is why
/// genuine fan-out waits for B rather than being faked here.)
pub fn fire(root: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver, host: &mut impl Host) {
    // Eager: settle first. Nothing is performed on a partial result.
    let Some(payload) = eval(root, inputs, resolver) else { return };
    // The manifest's sink capabilities are the effects whose consequence exits here.
    let caps = capabilities(root, resolver);
    for cap in caps.iter() {
        if cap.kind() == Kind::Sink {
            host.perform(cap, &payload);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{Edge, MapResolver, Param, Port, Type, UnitBody, BOUNDARY};

    /// A recording host: the test double for the platform. Captures what *would* be
    /// performed, so the loop is testable without touching the world.
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
    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn bare_sink_is_performed_once_with_settled_payload() {
        // A bare email leaf: eval settles its payload (identity), and the host performs
        // the egress exactly once with that settled value — only the host, never core.
        let r = MapResolver::new();
        let mut host = Recorder::default();
        fire(&email(), &[Some(text("hi"))], &r, &mut host);
        assert_eq!(host.performed, vec![(Capability::NetworkEgress, text("hi"))]);
    }

    #[test]
    fn sink_in_a_composite_is_performed() {
        // The effect one level in: input → email → output. The settled payload reaches
        // the output (consequences exit at the bottom); the host performs it.
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
        // No sink in the manifest ⇒ the host is never called. The pure surface performs
        // no effects; only an egress / write / spawn does.
        let r = MapResolver::new();
        let constant = Unit {
            body: UnitBody::Primitive { name: "number".into() },
            params: vec![Param::frozen("value", "5")],
            inputs: vec![],
            output: Some(Type::Number),
        };
        let mut host = Recorder::default();
        fire(&constant, &[], &r, &mut host);
        assert!(host.performed.is_empty());
    }
}