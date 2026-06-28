//! Determinism harness (P1.4.4) — the precondition the shared cache (P3.3) stands on.
//!
//! The evaluator already uses only **ordered** structures (`BTreeMap`/`BTreeSet`,
//! index-keyed scratch) — never `HashMap` iteration — so identical `(unit, inputs)`
//! yield identical output. This module *proves* that (a golden value plus a randomized
//! cross-check against a direct computation) and pins the **float discipline**: the
//! value domain admits only finite numbers, so equality and (later, P3.3) canonical
//! hashing are total and stable.
//!
//! Cross-target (native vs WASM) identity is the other half of I5; it is exercised once
//! the WASM bindings exist (P4.2.1). Here we establish single-target determinism.

/// The value domain admits only **finite** numbers; NaN and ±inf are excluded. The
/// `number` constant rejects anything failing this, so non-finite values never enter
/// from user data. (Arithmetic on finite inputs with guarded division stays finite for
/// well-formed programs.)
pub fn is_finite_number(x: f64) -> bool {
    x.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eval, Edge, Hash, MapResolver, Param, Port, Resolver, Type, Unit, UnitBody, Value, BOUNDARY};

    fn constant(v: i64) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "number".into() },
            params: vec![Param { name: "value".into(), value: v.to_string() }],
            inputs: vec![],
            output: Some(Type::Number),
        }
    }
    fn prim(name: &str, n_in: usize) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: (0..n_in).map(|i| Port { name: format!("i{i}"), ty: Type::Any }).collect(),
            output: Some(Type::Number),
        }
    }
    fn closed(children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs: vec![], output: Some(Type::Number) }
    }
    fn e(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }
    /// children 0 and 1 feed the op at child 2; the op's output is exposed.
    fn binop_edges() -> Vec<Edge> {
        vec![e(0, 0, 2, 0), e(1, 0, 2, 1), e(2, 0, BOUNDARY, 0)]
    }

    #[test]
    fn golden_nested_arithmetic() {
        // (2 + 3) * (10 - 4) = 30
        let mut r = MapResolver::new();
        let h2 = r.insert(constant(2)).unwrap();
        let h3 = r.insert(constant(3)).unwrap();
        let h10 = r.insert(constant(10)).unwrap();
        let h4 = r.insert(constant(4)).unwrap();
        let hadd = r.insert(prim("add", 2)).unwrap();
        let hsub = r.insert(prim("subtract", 2)).unwrap();
        let hmul = r.insert(prim("multiply", 2)).unwrap();
        let hleft = r.insert(closed(vec![h2, h3, hadd], binop_edges())).unwrap();
        let hright = r.insert(closed(vec![h10, h4, hsub], binop_edges())).unwrap();
        let top = closed(vec![hleft, hright, hmul], binop_edges());
        assert_eq!(eval(&top, &[], &r), Some(Value::Number(30.0)));
    }

    /// Tiny LCG for reproducible random graphs — the pure core takes no external deps.
    struct Lcg(u64);
    impl Lcg {
        fn new(seed: u64) -> Self {
            Lcg(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) % n
        }
    }

    /// Build a random arithmetic tree of finite integers; return its hash and the exact
    /// expected value computed directly. Depth ≤ 4 keeps every result an exact integer
    /// in `f64` (≤ 5^16 < 2^53), so eval must match the reference bit-for-bit.
    fn build(rng: &mut Lcg, depth: u32, r: &mut MapResolver) -> (Hash, f64) {
        if depth == 0 || rng.below(10) < 4 {
            let v = rng.below(11) as i64 - 5; // -5..=5
            (r.insert(constant(v)).unwrap(), v as f64)
        } else {
            let (lh, le) = build(rng, depth - 1, r);
            let (rh, re) = build(rng, depth - 1, r);
            let (name, exp) = match rng.below(3) {
                0 => ("add", le + re),
                1 => ("subtract", le - re),
                _ => ("multiply", le * re),
            };
            let oph = r.insert(prim(name, 2)).unwrap();
            (r.insert(closed(vec![lh, rh, oph], binop_edges())).unwrap(), exp)
        }
    }

    #[test]
    fn deterministic_over_random_graphs() {
        for seed in 0..200u64 {
            let mut rng = Lcg::new(seed);
            let mut r = MapResolver::new();
            let (h, exp) = build(&mut rng, 4, &mut r);
            let top = r.resolve(&h).unwrap().clone();
            let a = eval(&top, &[], &r);
            let b = eval(&top, &[], &r);
            assert_eq!(a, b, "non-deterministic at seed {seed}");
            assert_eq!(a, Some(Value::Number(exp)), "wrong value at seed {seed}");
        }
    }

    #[test]
    fn non_finite_constants_rejected() {
        assert!(is_finite_number(3.5));
        assert!(!is_finite_number(f64::NAN));
        assert!(!is_finite_number(f64::INFINITY));

        use crate::primitives::compute;
        let p = |v: &str| vec![Param { name: "value".into(), value: v.into() }];
        assert_eq!(compute("number", &[], &p("NaN")), None);
        assert_eq!(compute("number", &[], &p("inf")), None);
        assert_eq!(compute("number", &[], &p("2.5")), Some(Value::Number(2.5)));
    }
}