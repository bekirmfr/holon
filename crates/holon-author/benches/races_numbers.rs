//! Numbers: each gap of JavaScript's `Math`, `Number` and numeric operators, a composite against a
//! native candidate (docs/engineering/themes/javascript-library.md §11, "Numbers").

mod race;

use holon_core::Value;
use race::{check, function, mapped, num, numbers, race, Side};

fn abs(xs: &[&Value]) -> Option<Value> {
    let [Value::Number(x)] = xs else { return None };
    Some(Value::Number(x.abs()))
}

fn main() {
    // Math.abs: `0 - x` at and below zero keeps `-0` out (`x * -1` would give `-0` for `0`).
    const ABS: &str = "function abs(x) {\n  return x <= 0 ? 0 - x : x\n}";
    let abs_c = function(ABS, "abs", 1);
    let edges: Vec<Vec<Value>> = [-3.5, -0.0, 0.0, 2.0, -1e308, 5e-324].iter().map(|&x| vec![num(x)]).collect();
    check("abs", &Side::Composite(&abs_c), &Side::Native(abs), &edges);
    race("abs", &Side::Composite(&abs_c), &Side::Native(abs), &[1], |_| vec![num(-3.5)]);
    let abs_m = mapped(ABS, "abs");
    race("abs, mapped", &Side::Composite(&abs_m), &Side::NativeMapped(abs), &[1, 64, 4096], |n| vec![numbers(n)]);
}
