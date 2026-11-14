// Every pure leaf over values a request or a flow can hand it, from the security pass of the
// JavaScript library's leaves: no panic, the same answer twice, every number it answers finite,
// a count past what a leaf may make refused at once, and nothing nested past the bound a value may
// nest to, on a thread a host's size. Each family value is a leaf of its own; each higher-order
// leaf is given functions of every arity, an inconsistent comparator among them. The values:
// numbers at the edges of JavaScript's (huge counts, -0, 2^53, the smallest denormal), texts with
// combining marks, astral characters and broken escapes, and lists and records, nested and not,
// as deep as a value may be among them.
//
// Run: cargo test -p holon-core --test audit_leaves -- --nocapture (prints the slowest leaves);
//      HOLON_AUDIT_TRACE=1 names each evaluation before it runs, so the last one named is the one
//      that overflowed the stack (which aborts the process before any report).

use holon_core::{capabilities_of, encode_value, eval, hash_unit, hex, interface_of, Edge, EncodeError, MapResolver, Param, Port, Type, Unit, UnitBody, Value, BOUNDARY, FAMILIES, GENESIS, MAX_BYTES, MAX_ITEMS, MAX_TEXT, MAX_VALUE_DEPTH, PASSES_EXTRA};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

/// Counts what each thread holds and the most it has held, as `holon_core::memory::Counting`
/// does, and refuses (the process aborts, naming the size) an allocation past what a thread may
/// hold (`MOST`): what the memory sweep measures each leaf with, a leaf that would make
/// gigabytes stopped before it does.
struct Peak;
thread_local! {
    static HELD: Cell<i64> = const { Cell::new(0) };
    static PEAK: Cell<i64> = const { Cell::new(0) };
    static MOST: Cell<i64> = const { Cell::new(i64::MAX) };
}
fn grow(bytes: i64) -> bool {
    HELD.try_with(|held| {
        let now = held.get() + bytes;
        if bytes > 0 && now > MOST.with(Cell::get) {
            return false;
        }
        held.set(now);
        PEAK.with(|peak| peak.set(peak.get().max(now)));
        true
    })
    .unwrap_or(true)
}
unsafe impl GlobalAlloc for Peak {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !grow(layout.size() as i64) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            grow(-(layout.size() as i64));
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        grow(-(layout.size() as i64));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let more = new_size as i64 - layout.size() as i64;
        if !grow(more) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            grow(-more);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Peak = Peak;

struct Rng(u64);
impl Rng {
    fn below(&mut self, m: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % m.max(1) as u64) as usize
    }
}

fn num(n: f64) -> Value {
    Value::Number(n)
}
fn txt(s: &str) -> Value {
    Value::Text(s.into())
}
fn rec(fs: &[(&str, Value)]) -> Value {
    Value::Record(fs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}
fn nested(depth: usize) -> Value {
    (0..depth).fold(num(1.0), |v, _| Value::List(vec![v, num(2.0)]))
}

fn pool() -> Vec<Value> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let numbers: Vec<Value> = (0..1000).map(|_| num(rng.below(2000) as f64 - 1000.0)).collect();
    let mixed: Vec<Value> = (0..200).map(|i| [num(i as f64), txt(&format!("t{}", i % 7)), Value::Boolean(i % 2 == 0), Value::List(vec![]), rec(&[])][i % 5].clone()).collect();
    vec![
        num(0.0),
        num(-0.0),
        num(1.0),
        num(-1.0),
        num(2.5),
        num(-7.5),
        num(3.0),
        num(36.0),
        num(100.0),
        num(1e21),
        num(-1e21),
        num(2147483648.0),
        num(4294967296.0),
        num(9007199254740994.0),
        num(1e9),
        num(1e300),
        num(f64::MAX),
        num(5e-324),
        txt(""),
        txt("a"),
        txt("é\u{301}ﬃ"),
        txt("a,b,,c"),
        txt("😀x😀"),
        txt("  padded\t\n"),
        txt("%E0%A4%A"),
        txt("%"),
        txt("2024-02-29T12:34:56.789Z"),
        txt("-0x1F"),
        txt("1e400"),
        txt("{\"a\":[1,{\"b\":null}],\"__proto__\":2}"),
        txt(&"ab😀".repeat(3000)),
        // Texts that hex and base64 decode, in each alphabet, and padding alone.
        txt("00ff7f80"),
        txt("aGVsbG8gd29ybGQ="),
        txt("_-8"),
        txt("===="),
        Value::Boolean(true),
        Value::Boolean(false),
        Value::List(vec![]),
        Value::List(vec![num(3.0), num(1.0), num(2.0), num(-0.0), num(0.0)]),
        Value::List(vec![txt("b"), txt("a"), txt("b")]),
        Value::List(vec![Value::List(vec![txt("a"), num(1.0)]), Value::List(vec![txt("b"), num(2.0)])]),
        Value::List(numbers),
        Value::List(mixed),
        nested(70),
        // As deep as a value may be: a leaf putting it a level further down gives nothing.
        nested(MAX_VALUE_DEPTH),
        (0..MAX_VALUE_DEPTH).fold(num(1.0), |v, _| rec(&[("a", v), ("b", num(2.0))])),
        rec(&[]),
        rec(&[("a", num(1.0)), ("b", txt("x")), ("c", Value::List(vec![num(1.0)]))]),
        rec(&[("a", rec(&[("a", rec(&[("a", num(0.0))]))]))]),
        Value::Sum { arm: 0, payload: Box::new(num(1.0)) },
        Value::Effect { sink: "store_write".into(), payload: Box::new(num(1.0)) },
    ]
    .into_iter()
    .chain(bytes())
    .collect()
}

/// Bytes a file, a request or another leaf can hand a leaf: none, odd ones, UTF-8 that isn't, the
/// starts of a gzip, a zip and an OpenPGP packet with lengths past anything, a gzip that inflates
/// past what a leaf may make (64 MiB and a byte of zeros, in one member: refused inside it, in a
/// tenth of a second unoptimised; members each within it and together past it are
/// compress.rs's `inflating_stops_at_its_budget`, as checking each one's checksum first takes a
/// second here), a zip and a gzip that are whole, and noise.
fn bytes() -> Vec<Value> {
    let b = |v: Vec<u8>| Value::Bytes(v);
    let made = |leaf: &str, input: Value| match holon_core::primitives::compute(leaf, &[&input], &[]) {
        Some(Value::Bytes(v)) => v,
        other => panic!("{leaf} gave {other:?}"),
    };
    let mut rng = Rng(0x5851_f42d_4c95_7f2d);
    vec![
        b(vec![]),
        b(vec![0, 1, 0x7f, 0x80, 0xff]),
        b(vec![0xc3, 0x28, 0xf0, 0x9f, 0x98]),
        b("héllo".as_bytes().to_vec()),
        b(vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 0x01]),
        b(made("gzip", b(vec![0; (64 << 20) + 1]))),
        b(made("gzip", txt("hello"))),
        b(b"PK\x03\x04\x14\0\0\0\x08\0".to_vec()),
        b([b"PK\x05\x06\0\0\0\0".as_slice(), &[0xff; 12], &[0, 0]].concat()),
        b(made("zip", rec(&[("a.txt", txt("hello")), ("b/c.bin", b(vec![1, 2, 3]))]))),
        b(vec![0x99, 0xff, 0xff]),
        b(vec![0xc6, 0xff, 0xff, 0xff, 0xff, 0xff]),
        b(b"-----BEGIN PGP MESSAGE-----\n\nwcBMA0\n=AAAA\n-----END PGP MESSAGE-----\n".to_vec()),
        b((0..4096).map(|_| rng.below(256) as u8).collect()),
        b(vec![0xaa; 64 << 10]),
    ]
}

fn port(name: &str) -> Port {
    Port { name: name.into(), ty: Type::Any }
}

/// Functions for the higher-order leaves, in `r`: the leaves of one and two inputs a flow most
/// often passes, a comparator answering booleans (not a total order), one whose answer is its
/// first input whole, and one of three inputs (an arity the caller doesn't hand).
fn functions(r: &mut MapResolver) -> Vec<String> {
    let mut leaf = |name: &str, n: usize, params: Vec<Param>| {
        let mut unit = interface_of(name).unwrap();
        unit.inputs = (0..n).map(|i| port(&format!("i{i}"))).collect();
        if !params.is_empty() {
            unit.params = params;
        }
        hex(r.insert(unit).unwrap())
    };
    let mut fns = vec![
        leaf("not", 1, vec![]),
        leaf("is_truthy", 1, vec![]),
        leaf("to_text", 1, vec![]),
        leaf("length", 1, vec![]),
        leaf("add", 2, vec![]),
        leaf("subtract", 2, vec![]),
        leaf("compare", 2, vec![Param::frozen("op", "<")]),
        leaf("pair", 2, vec![]),
        leaf("list", 3, vec![]),
    ];
    let first = Unit {
        body: UnitBody::Composite { children: vec![], edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
        params: vec![],
        inputs: vec![port("a"), port("b")],
        outputs: vec![port("out")],
    };
    fns.push(hex(r.insert(first).unwrap()));
    fns
}

/// Every pure leaf, once for each value of its family parameters and each function a
/// higher-order one may be given, with its frozen parameters' odd values too.
fn variants(fns: &[String]) -> Vec<(String, Unit)> {
    let mut out = Vec::new();
    for &name in GENESIS {
        let Some(base) = interface_of(name) else { continue };
        if !capabilities_of(name, &base.params).is_some_and(|c| c.is_pure()) {
            continue;
        }
        let mut units = vec![base.clone()];
        for (_, param, values) in FAMILIES.iter().filter(|(leaf, _, _)| *leaf == name) {
            units = units
                .iter()
                .flat_map(|u| values.iter().map(move |v| set(u, param, v)))
                .collect();
        }
        let odd: &[(&str, &[&str])] = match name {
            "flat" => &[("depth", &["0", "1", "3", "Infinity", "1e9", "-1", "x"])],
            "json_stringify" => &[("indent", &["", "  ", "\t", "abcdefghijklmnop"]), ("keys", &["", "[\"a\",\"b\"]", "[1]", "["])],
            "field" | "with_field" | "without_field" => &[("name", &["a", "__proto__", ""])],
            "sha256" | "hmac_sha256" => &[("encoding", &["hex", "base64", "x"])],
            // Their mode isn't a family, so each way is named here: decoding, too.
            "hex" => &[("mode", &["encode", "decode", "x"])],
            "base64" => &[("mode", &["encode", "decode"]), ("alphabet", &["standard", "url", "x"]), ("bytes", &["utf8", "latin1"])],
            _ => &[],
        };
        for (param, values) in odd {
            units = units.iter().flat_map(|u| values.iter().map(move |v| set(u, param, v))).collect();
        }
        if PASSES_EXTRA.contains(&name) {
            units = units.iter().flat_map(|u| fns.iter().map(move |f| set(u, "fn", f))).collect();
            if matches!(name, "json_parse" | "json_stringify") {
                units.push(base.clone());
            }
        }
        for u in units {
            let label = format!("{name}{}", u.params.iter().map(|p| format!(" {}={}", p.name(), short(p.value().unwrap_or("")))).collect::<String>());
            out.push((label, u));
        }
    }
    out
}

fn set(unit: &Unit, param: &str, value: &str) -> Unit {
    let mut u = unit.clone();
    u.params.retain(|p| p.name() != param);
    u.params.push(Param::frozen(param, value));
    u.params.sort_by(|a, b| a.name().cmp(b.name()));
    u
}

fn short(s: &str) -> String {
    s.chars().take(10).collect()
}

/// Every number in `v` is finite, as a value a run carries must be.
fn finite(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_finite(),
        Value::List(xs) => xs.iter().all(finite),
        Value::Record(fs) => fs.values().all(finite),
        Value::Sum { payload, .. } | Value::Effect { payload, .. } => finite(payload),
        _ => true,
    }
}

fn input_sets(n: usize, pool: &[Value], rng: &mut Rng) -> Vec<Vec<Option<Value>>> {
    let pick = |i: usize| if i == pool.len() { None } else { Some(pool[i].clone()) };
    match n {
        0 => vec![vec![]],
        1 => (0..=pool.len()).map(|i| vec![pick(i)]).collect(),
        2 => (0..=pool.len()).flat_map(|i| (0..=pool.len()).map(move |j| (i, j))).map(|(i, j)| vec![pick(i), pick(j)]).collect(),
        _ => (0..1500).map(|_| (0..n).map(|_| pick(rng.below(pool.len() + 1))).collect()).collect(),
    }
}

/// **Guards:** every pure leaf, in every form its parameters give it, over every value of the pool
/// alone and in pairs (and drawn sets of three or more): it answers, the same twice, every number
/// in its answer finite, nothing nested past the bound, in bounded time.
///
/// **Why:** a leaf's inputs are anyone's: a request's body, a file, a message, another leaf's
/// answer. A leaf is code the host runs on its own thread, so one that panics, loops, answers a
/// non-finite number or makes a value past the bounds takes more than its run with it.
///
/// **If it fails:** the report names each leaf and the inputs it misbehaved on; a request
/// shaped like them could take the server down or hold a thread, so the leaf is fixed, not the
/// pool.
#[test]
fn every_pure_leaf_over_hostile_values_answers_finitely_and_the_same_twice() {
    // A host runs a flow on a 2 MiB thread (tokio's); the sweep runs on one too.
    let found = std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(sweep).unwrap().join().unwrap();
    assert!(found.is_empty(), "{} leaves misbehaved:\n{}", found.len(), found.join("\n"));
}

/// A number that isn't finite reaches a leaf from a module's answer (a `js` function's `1 / 0`),
/// never from a leaf's own. Over NaN and the infinities, alone and inside a list and a record,
/// beside ordinary values, no pure leaf panics. What a leaf answers isn't held to the rest of the
/// sweep here: NaN equals nothing, itself included, and a leaf that hands its input on hands that
/// on. The panic hook is left alone, so a panic says where.
#[test]
fn no_pure_leaf_panics_on_a_number_that_isnt_finite() {
    let found = std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(sweep_not_finite).unwrap().join().unwrap();
    assert!(found.is_empty(), "{} leaves panicked:\n{}", found.len(), found.join("\n"));
}

fn sweep_not_finite() -> Vec<String> {
    let mut r = MapResolver::new();
    let fns = functions(&mut r);
    let odd = [
        num(f64::NAN),
        num(f64::INFINITY),
        num(f64::NEG_INFINITY),
        Value::List(vec![num(1.0), num(f64::NAN), num(f64::INFINITY)]),
        rec(&[("a", num(f64::NEG_INFINITY)), ("b", num(f64::NAN))]),
    ];
    let plain = [num(0.0), num(2.0), num(-7.5), num(36.0), txt(""), txt("a,b"), Value::Boolean(true), Value::List(vec![num(3.0), txt("b")]), rec(&[("a", num(1.0))])];
    let pool: Vec<Value> = odd.iter().chain(&plain).cloned().collect();
    let mut rng = Rng(0x6c62_272e_07bb_0142);
    let mut found = Vec::new();
    for (label, unit) in variants(&fns) {
        // Every set holds one of the odd values at least.
        let sets: Vec<Vec<Option<Value>>> = match unit.inputs.len() {
            0 => continue,
            1 => odd.iter().map(|v| vec![Some(v.clone())]).collect(),
            2 => (0..pool.len())
                .flat_map(|i| (0..pool.len()).map(move |j| (i, j)))
                .filter(|(i, j)| *i < odd.len() || *j < odd.len())
                .map(|(i, j)| vec![Some(pool[i].clone()), Some(pool[j].clone())])
                .collect(),
            n => (0..300)
                .map(|_| {
                    let mut set: Vec<Option<Value>> = (0..n).map(|_| Some(pool[rng.below(pool.len())].clone())).collect();
                    set[rng.below(n)] = Some(odd[rng.below(odd.len())].clone());
                    set
                })
                .collect(),
        };
        let mut panicked = (0usize, String::new());
        for inputs in sets {
            if std::env::var_os("HOLON_AUDIT_TRACE").is_some() {
                eprintln!("{label} <- {}", inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", "));
            }
            if catch_unwind(AssertUnwindSafe(|| eval(&unit, &inputs, &r).into_vec())).is_err() {
                if panicked.0 == 0 {
                    panicked.1 = inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", ");
                }
                panicked.0 += 1;
            }
        }
        if panicked.0 > 0 {
            found.push(format!("{label}: panics on {} input sets, e.g. ({})", panicked.0, panicked.1));
        }
    }
    found
}

fn sweep() -> Vec<String> {
    let mut r = MapResolver::new();
    let fns = functions(&mut r);
    let pool = pool();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut found = Vec::new();
    let mut slowest: Vec<(Duration, String)> = Vec::new();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let variants = variants(&fns);
    let mut evals = 0usize;
    for (label, unit) in &variants {
        assert!(hash_unit(unit).is_ok(), "{label} hashes");
        let mut worst = (Duration::ZERO, String::new());
        let mut faults: BTreeMap<&str, (usize, String)> = BTreeMap::new();
        for inputs in input_sets(unit.inputs.len(), &pool, &mut rng) {
            evals += 1;
            if std::env::var_os("HOLON_AUDIT_TRACE").is_some() {
                eprintln!("{label} <- {}", inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", "));
            }
            let started = Instant::now();
            let first = catch_unwind(AssertUnwindSafe(|| eval(unit, &inputs, &r).into_vec()));
            let took = started.elapsed();
            let shown = || inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", ");
            if took > worst.0 {
                worst = (took, shown());
            }
            let fault = match first {
                Err(_) => Some("panics"),
                Ok(out) if !out.iter().flatten().all(finite) => Some("answers a number that isn't finite"),
                Ok(out) if out.iter().flatten().any(|v| encode_value(v) == Err(EncodeError::TooDeep)) => Some("answers a value nested past the bound"),
                Ok(out) => match catch_unwind(AssertUnwindSafe(|| eval(unit, &inputs, &r).into_vec())) {
                    Ok(again) if again == out => None,
                    _ => Some("answers differently the second time"),
                },
            };
            let fault = fault.or((took > Duration::from_secs(2)).then_some("takes more than 2 seconds"));
            if let Some(f) = fault {
                faults.entry(f).or_insert((0, shown())).0 += 1;
            }
        }
        for (fault, (count, example)) in faults {
            found.push(format!("{label}: {fault} on {count} input sets, e.g. ({example})"));
        }
        slowest.push((worst.0, format!("{label} ({})", worst.1)));
    }
    std::panic::set_hook(hook);
    slowest.sort_by_key(|s| std::cmp::Reverse(s.0));
    println!("{} leaves, {evals} evaluations; the slowest:", variants.len());
    for (took, what) in slowest.iter().take(12) {
        println!("  {took:>10.2?}  {what}");
    }
    found
}

/// The bytes `v` holds on the heap, as its representation lays it out: a list's items and a
/// record's entries each a value wide, and what each holds besides.
fn deep(v: &Value) -> usize {
    const WIDE: usize = std::mem::size_of::<Value>();
    match v {
        Value::Text(s) => s.len(),
        Value::Bytes(b) => b.len(),
        Value::List(xs) => xs.iter().map(|x| WIDE + deep(x)).sum(),
        Value::Record(fs) => fs.iter().map(|(k, x)| k.len() + 3 * WIDE + deep(x)).sum(),
        Value::Sum { payload, .. } | Value::Effect { payload, .. } => WIDE + deep(payload),
        _ => 0,
    }
}

/// Values as large as a request's or a file's, and the counts and separators a leaf makes many of
/// them with: texts of 64 KiB of each kind a leaf makes more of (three-byte characters, one that
/// normalizes to eighteen, controls JSON escapes, commas, JSON, XML, `ß` that upper-cases to two),
/// bytes, lists and a record as large, a gzip and a zip that inflate to the most they may, and
/// the counts (up to `MAX_TEXT`) and separators a leaf repeats.
fn large() -> Vec<Value> {
    let n = 1 << 16;
    let json = format!("[{}0]", "0,".repeat(n / 2));
    let xml = format!("<a>{}</a>", "<b/>".repeat(n / 4));
    let made = |leaf: &str, input: Value| holon_core::primitives::compute(leaf, &[&input], &[]).unwrap_or_else(|| panic!("{leaf}"));
    vec![
        txt(&"a".repeat(n)),
        txt(&"\u{20ac}".repeat(n / 3)),
        txt(&"\u{fdfa}".repeat(n / 3)),
        txt(&"\u{0}".repeat(n)),
        txt(&",".repeat(n)),
        txt(&"a,".repeat(n / 2)),
        txt(&json),
        txt(&xml),
        txt(&"\u{df}".repeat(n / 2)),
        Value::Bytes(vec![0; n]),
        Value::Bytes(vec![0xff; n]),
        made("gzip", Value::Bytes(vec![0; MAX_BYTES])),
        made("zip", rec(&[("a", Value::Bytes(vec![0; MAX_BYTES]))])),
        Value::List((0..n / 16).map(|i| num(i as f64)).collect()),
        Value::List((0..64).map(|i| txt(&format!("{i}").repeat(64))).collect()),
        Value::List((0..64).map(|i| Value::List(vec![num(i as f64); 64])).collect()),
        Value::Record((0..n / 32).map(|i| (format!("k{i}"), num(i as f64))).collect()),
        // Small: what a leaf repeats, joins or counts by.
        txt(""),
        txt(","),
        txt("ab"),
        txt(&"x".repeat(4096)),
        num(0.0),
        num(1.0),
        num(3.0),
        num(1000.0),
        num(65536.0),
        num(MAX_TEXT as f64),
        num(1e9),
        Value::Boolean(true),
        Value::List(vec![]),
        rec(&[]),
    ]
}

/// What a step of a leaf may make beyond its inputs: sixteen times what it is handed, and a
/// little (a table, a buffer).
const TIMES: usize = 16;
const SLACK: usize = 64 << 10;

/// The leaves that make more from what they are handed, each with the most it makes, which a bound
/// of its own sets (`no_leaf_makes_a_value_past_the_most_one_may_be` in primitives.rs holds them to
/// it at that bound; gzip's and zip's are compress.rs's).
fn allowance(leaf: &str) -> usize {
    match leaf {
        // A text of a count, a separator or a replacement: up to MAX_TEXT units, three bytes each.
        "pad" | "repeat" | "replace" | "join" => 3 * MAX_TEXT,
        // Copies of a value, or bytes inflated: up to MAX_BYTES.
        "fill" | "gunzip" | "unzip" => MAX_BYTES,
        // Values read out of one text or bytes: up to MAX_ITEMS, a hundred bytes or so each while
        // they are read.
        "split" | "json_parse" | "csv_parse" | "xml_parse" | "bytes_to_list" => 128 * MAX_ITEMS,
        _ => 0,
    }
}

/// Measures what each step of an evaluation makes: the most the thread came to hold, from what it
/// held as the step began, until the next step began. A step at a time (`take_steps` gives one),
/// so a higher-order leaf's applications are each measured, as a host checks each.
struct PerStep {
    /// What the thread held as the step began.
    from: i64,
    /// The most a step made.
    most: i64,
}
impl PerStep {
    fn new() -> PerStep {
        let from = HELD.with(Cell::get);
        PEAK.with(|p| p.set(from));
        PerStep { from, most: 0 }
    }
    /// Ends the step under way: what it made counted, and the next one begins.
    fn end(&mut self) -> i64 {
        let held = HELD.with(Cell::get);
        self.most = self.most.max(PEAK.with(Cell::get) - self.from);
        self.from = held;
        PEAK.with(|p| p.set(held));
        self.most
    }
}
impl holon_core::Observer for PerStep {
    fn step(&mut self) -> bool {
        self.end();
        true
    }
}

/// Input sets for a leaf of `n` inputs over `pool`: each value alone and every pair, as
/// [`input_sets`]; past two, every pair of values at every pair of ports, the other ports drawn.
fn sets_of_large(n: usize, pool: &[Value], rng: &mut Rng) -> Vec<Vec<Option<Value>>> {
    if n < 3 {
        return input_sets(n, pool, rng);
    }
    let mut sets = Vec::new();
    for a in 0..n {
        for b in a + 1..n {
            for x in pool {
                for y in pool {
                    let mut set: Vec<Option<Value>> = (0..n).map(|_| Some(pool[rng.below(pool.len())].clone())).collect();
                    set[a] = Some(x.clone());
                    set[b] = Some(y.clone());
                    sets.push(set);
                }
            }
        }
    }
    sets
}

/// What each pure leaf makes at most in one step, over [`large`] values, and what it was handed:
/// the worst of each leaf.
fn sweep_memory() -> Vec<(String, usize, usize, String)> {
    let mut r = MapResolver::new();
    let fns = functions(&mut r);
    let pool = large();
    let mut rng = Rng(0x1f83_d9ab_fb41_bd6b);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut worst = Vec::new();
    for (label, unit) in variants(&fns) {
        let name = match &unit.body {
            UnitBody::Primitive { name } => name.clone(),
            _ => unreachable!(),
        };
        let mut most = (0usize, 0usize, String::new());
        for inputs in sets_of_large(unit.inputs.len(), &pool, &mut rng) {
            if std::env::var_os("HOLON_AUDIT_TRACE").is_some() {
                eprintln!("{label} <- {}", inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", "));
            }
            let given: usize = inputs.iter().flatten().map(deep).sum();
            let mut steps = PerStep::new();
            MOST.with(|m| m.set(HELD.with(Cell::get) + (1 << 30)));
            let out = catch_unwind(AssertUnwindSafe(|| holon_core::eval_observed(&unit, &inputs, &r, &mut steps).into_vec()));
            let made = steps.end().max(0) as usize;
            MOST.with(|m| m.set(i64::MAX));
            drop(out);
            if made > most.0 {
                let shown = inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", ");
                most = (made, given, shown);
            }
        }
        worst.push((name, most.0, most.1, format!("{label} ({})", most.2)));
    }
    std::panic::set_hook(hook);
    worst.sort_by_key(|w| std::cmp::Reverse(w.1));
    worst
}

/// **Guards:** no step of any pure leaf, in any form its parameters give it, over large values alone,
/// in pairs and at every pair of its ports, makes more than sixteen times what it is handed (and
/// 64 KiB), except the leaves named with the bound that holds what they make (`allowance`). A step
/// is measured from its start to the next's, so a higher-order leaf's applications are each one.
///
/// **Why:** a round's memory (`max_memory`) is looked at before each step, so what one step makes
/// is what a round may hold past it. A leaf that makes far more than it is handed, a value copied
/// into each of many items or a separator between them, makes gigabytes of a request's megabytes
/// in one step, and the process aborts, taking every deployment with it.
///
/// **If it fails:** the report names each leaf, what it made and what it was handed. Bound what it
/// makes before it makes it (as `sized` in primitives.rs does), and name it with that bound here;
/// a leaf that makes some multiple of its input more than this is checked for a bound of its own
/// too.
#[test]
fn no_step_of_a_pure_leaf_makes_more_than_sixteen_times_what_it_is_handed_or_its_bound() {
    let worst = std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(sweep_memory).unwrap().join().unwrap();
    for (_, made, given, what) in worst.iter().take(12) {
        println!("{:>8.1} MiB  given {:>8.1} KiB  {what}", *made as f64 / 1048576.0, *given as f64 / 1024.0);
    }
    let over: Vec<String> = worst
        .iter()
        .filter(|(leaf, made, given, _)| *made > TIMES * given + SLACK + allowance(leaf))
        .map(|(_, made, given, what)| format!("{what}: made {made} bytes, handed {given}"))
        .collect();
    assert!(over.is_empty(), "{} leaves made more than their bound in a step:\n{}", over.len(), over.join("\n"));
}
