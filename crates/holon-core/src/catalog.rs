//! The authoritative interface of every genesis primitive.
//!
//! Core already maps a primitive *name* to two properties, each in one place: its **behaviour**
//! ([`crate::primitives::compute`]) and its **capability** ([`crate::capabilities_of`]). This is the
//! third axis — a name → its typed **interface** (named input ports, output type, default params) —
//! so the genesis set is defined *by code*, not re-asserted by hand in every test and (next) the
//! editor's palette. Same fail-closed contract: an unknown name yields `None`.
//!
//! The 34 leaves and their grouping are §13a of the design doc. The *types* are read off `compute`'s
//! arity/coercions and `eval`'s higher-order handling: variadic leaves (`add`, `and`, `concat`,
//! `list`, …) get their canonical arity 2 (true variadic editing is a later editor concern);
//! polymorphic leaves use [`Type::Any`] (the wiring check already treats `Any` as compatible);
//! `map`/`fold` carry their function as an **open** `fn` param (the editor binds it), matching the
//! evaluator. `interface_of` returns a *default-configured* unit (`const` at `Number 0`, `compare` at
//! `op="eq"`) so the palette drops a usable, valid node. See primitive-catalog-L4.

use crate::{Param, Port, Type, Unit, UnitBody};

/// Every genesis primitive name (§13a), in manifest order. The enumeration source for the catalog.
pub const GENESIS: &[&str] = &[
    // pure — language core (16)
    "const", "add", "multiply", "subtract", "divide", "compare", "and", "or", "not", "select",
    "cons", "head", "tail", "is_empty", "fold", "map",
    // pure — text (6)
    "concat", "uppercase", "lowercase", "text_length", "slice", "index_of",
    // pure — list convenience, demotion-deferred (6)
    "list", "get", "append", "length", "sum", "join",
    // drivers — sourced, no compute (3)
    "clock", "random", "ingress",
    // state cell (2)
    "store_read", "store_write",
    // sink — performed (1)
    "egress",
];

/// The canonical genesis interface for `name`, or `None` if it is not a genesis primitive
/// (fail-closed, mirroring [`crate::capabilities_of`]).
pub fn interface_of(name: &str) -> Option<Unit> {
    use Type::*;
    // A primitive unit whose body name is the one being asked for.
    let prim = |inputs: Vec<Port>, params: Vec<Param>, output: Type| Unit {
        body: UnitBody::Primitive { name: name.to_string() },
        params,
        inputs,
        output: Some(output),
    };
    let port = |n: &str, ty: Type| Port { name: n.to_string(), ty };

    Some(match name {
        // ── pure — language core ──
        "const" => prim(
            vec![],
            vec![Param::frozen("value", "0"), Param::frozen("type", "Number")],
            Number,
        ),
        "add" | "multiply" | "subtract" | "divide" => {
            prim(vec![port("a", Number), port("b", Number)], vec![], Number)
        }
        "compare" => prim(
            vec![port("a", Number), port("b", Number)],
            vec![Param::frozen("op", "eq")],
            Boolean,
        ),
        "and" | "or" => prim(vec![port("a", Boolean), port("b", Boolean)], vec![], Boolean),
        "not" => prim(vec![port("v", Boolean)], vec![], Boolean),
        "select" => prim(
            vec![port("cond", Boolean), port("if_true", Any), port("if_false", Any)],
            vec![],
            Any,
        ),
        "cons" => prim(vec![port("item", Any), port("list", Type::list_any())], vec![], Type::list_any()),
        "head" => prim(vec![port("list", Type::list_any())], vec![], Any),
        "tail" => prim(vec![port("list", Type::list_any())], vec![], Type::list_any()),
        "is_empty" => prim(vec![port("list", Type::list_any())], vec![], Boolean),
        // higher-order: the function rides an open `fn` param (hex hash once bound), not a port.
        "fold" => prim(
            vec![port("init", Any), port("list", Type::list_any())],
            vec![Param::open("fn", "fn")],
            Any,
        ),
        "map" => prim(vec![port("list", Type::list_any())], vec![Param::open("fn", "fn")], Type::list_any()),

        // ── pure — text ──
        "concat" => prim(vec![port("a", Text), port("b", Text)], vec![], Text),
        "uppercase" | "lowercase" => prim(vec![port("s", Text)], vec![], Text),
        "text_length" => prim(vec![port("s", Text)], vec![], Number),
        "slice" => prim(
            vec![port("s", Text), port("start", Number), port("len", Number)],
            vec![],
            Text,
        ),
        "index_of" => prim(vec![port("haystack", Text), port("needle", Text)], vec![], Number),

        // ── pure — list convenience ──
        "list" => prim(vec![port("a", Any), port("b", Any)], vec![], Type::list_any()),
        "get" => prim(vec![port("list", Type::list_any()), port("index", Number)], vec![], Any),
        "append" => prim(vec![port("list", Type::list_any()), port("item", Any)], vec![], Type::list_any()),
        "length" => prim(vec![port("list", Type::list_any())], vec![], Number),
        "sum" => prim(vec![port("list", Type::list_any())], vec![], Number),
        "join" => prim(vec![port("list", Type::list_any()), port("sep", Text)], vec![], Text),

        // ── drivers — sourced (no inputs; host-supplied). Concrete where unambiguous, `Any` where
        //    the payload is genuinely polymorphic (the editor may specialize later). ──
        "clock" => prim(vec![], vec![], Number),
        "random" => prim(vec![], vec![], Number),
        "ingress" => prim(vec![], vec![], Any),

        // ── state cell ──
        "store_read" => prim(vec![], vec![], Any),
        "store_write" => prim(vec![port("value", Any)], vec![], Effect),

        // ── sink ──
        "egress" => prim(vec![port("payload", Any)], vec![], Effect),

        _ => return None,
    })
}

/// Every genesis primitive's canonical interface, in manifest order. A convenience over
/// `GENESIS` + [`interface_of`] (what the editor's palette will be derived from).
pub fn genesis_units() -> Vec<Unit> {
    GENESIS.iter().filter_map(|&n| interface_of(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capabilities_of, eval, MapResolver, Value};

    #[test]
    fn interface_of_unknown_is_none() {
        // Fail-closed, like capabilities_of: a name the engine doesn't vouch for has no interface.
        assert!(interface_of("while").is_none());
        assert!(interface_of("recurse").is_none());
        assert!(interface_of("").is_none());
    }

    #[test]
    fn genesis_has_34_leaves() {
        // The §13a count — a tripwire if the frozen set is ever changed by accident.
        assert_eq!(GENESIS.len(), 34);
    }

    #[test]
    fn every_genesis_name_resolves() {
        // Each enumerated name resolves, and the unit's body name matches it (no typo drift between
        // the GENESIS list and the interface_of match).
        for &n in GENESIS {
            let u = interface_of(n).unwrap_or_else(|| panic!("no interface for genesis name {n}"));
            match &u.body {
                UnitBody::Primitive { name } => assert_eq!(name, n),
                _ => panic!("{n} should be a primitive"),
            }
        }
        assert_eq!(genesis_units().len(), GENESIS.len());
    }

    #[test]
    fn the_two_catalogs_agree() {
        // The interface axis and the capability axis cover the same atom set — drift in either
        // (a name in one table but not the other) is caught here.
        for &n in GENESIS {
            assert!(capabilities_of(n).is_some(), "{n} missing from capabilities_of");
        }
    }

    #[test]
    fn catalogued_units_evaluate() {
        // Prove the interfaces are wired right by *running* them, not just trusting §13a prose:
        // the default const yields 0, and add/not compute as compute() does.
        let r = MapResolver::new();
        let n = |x: f64| Value::Number(x);

        let konst = interface_of("const").unwrap();
        assert_eq!(eval(&konst, &[], &r), Some(n(0.0)));

        let add = interface_of("add").unwrap();
        assert_eq!(eval(&add, &[Some(n(2.0)), Some(n(3.0))], &r), Some(n(5.0)));

        let not = interface_of("not").unwrap();
        assert_eq!(eval(&not, &[Some(Value::Boolean(true))], &r), Some(Value::Boolean(false)));
    }
}