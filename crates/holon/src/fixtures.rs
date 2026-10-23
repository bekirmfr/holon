//! Test units: the small units the CLI's fixture flags emit and the tests deploy.

use crate::*;

/// A trivial, valid unit for round-trip testing only — the same `const` shape already used as
/// a test helper elsewhere in this project (e.g. `holon-host`'s own test module), not a new
/// invention. It doesn't need to be evaluable for this purpose, just a real, canonically
/// encodable `Unit`.
pub(crate) fn test_unit() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("type", "Number"), Param::frozen("value", "42")],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// A one-child passthrough composite referencing `leaf_hash` — the same `pass_through` shape
/// used as a test helper in `holon-core`'s own `simulate.rs` tests, reused rather than
/// reinvented. Exists only to exercise `Registry::publish`'s `DanglingReference` path: publish
/// this *before* its leaf has been staged via `POST /unit`, and publish should refuse, naming
/// exactly `leaf_hash`.
pub(crate) fn test_composite(leaf_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![leaf_hash],
            edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// A second, distinct leaf (same shape as `test_unit()`, a different frozen value so its hash
/// differs) — for exercising `POST /registry/fork`: re-pin `test_composite`'s child from
/// `test_unit()`'s hash to this one's.
pub(crate) fn test_replacement() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "const".into() },
        params: vec![Param::frozen("type", "Number"), Param::frozen("value", "99")],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// The `egress` sink primitive — the same shape used as a test helper throughout `holon-core`
/// and `holon-host`'s own tests, reused rather than reinvented. Under Fork B a sink is a pure
/// request-constructor (`eval` settles it to `Value::Effect{sink:"egress",payload}` without any
/// IO), and `fire` is what actually performs whatever the settled output carries — so firing
/// this unit with a real message input is what finally exercises `RecordingHost::perform` and
/// gives `POST /fire`'s `performed` list something real to report.
pub(crate) fn test_sink() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "egress".into() },
        params: vec![],
        inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

/// The `ingress` driver primitive — zero inputs, output `Any` (its real genesis interface,
/// confirmed against `catalog.rs`, not assumed). For testing `POST /trigger/<slug>`: nothing
/// this session has fired a real driver leaf before, so this is a genuinely new shape, not a
/// variant of `test_sink`/`test_unit`.
pub(crate) fn test_ingress() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "ingress".into() },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Any }],
    }
}

/// `ingress → egress`: whatever `/trigger/<slug>` injects for `NetworkIngress` flows straight
/// through to `egress`'s payload. Exercises both halves of `POST /trigger/<slug>` at once —
/// the *sourced* substitution (this composite's own settled output should equal whatever was
/// sent) and `performed` (the `egress` sink should show up, same as `test_sink` alone proved
/// for `/fire`, but now reached via a real driver instead of a positional input).
pub(crate) fn test_ingress_flow(ingress_hash: Hash, egress_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![ingress_hash, egress_hash],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

/// The `clock` driver primitive — zero inputs, output `Number` (its real genesis interface;
/// unlike `ingress`, this one is concretely typed, not `Any`).
pub(crate) fn test_clock() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "clock".into() },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// `clock → egress`: wire-compatible because `egress`'s `payload` port is `Any` (`compatible`'s
/// rule: `dst==Any` accepts anything). For exercising `POST /deploy`'s derived-`Clock` path and
/// `spawn_clock_scheduler` end to end — every tick should record a `NetworkEgress` carrying
/// that tick's epoch-seconds value.
pub(crate) fn test_clock_flow(clock_hash: Hash, egress_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![clock_hash, egress_hash],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}
