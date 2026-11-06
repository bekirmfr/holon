//! Test units: the small units the CLI's fixture flags emit and the tests stage and fire.

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
