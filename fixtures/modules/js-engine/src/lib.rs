//! Holon's JavaScript engine module (docs/L4s/js-node-L4.md §4): Boa, compiled to a WebAssembly
//! module with **no imports**, behind the module ABI of docs/L4s/wasm-module-L4.md §3.
//!
//! Its interface, as a unit: `engine(source: Text, args: List<Any>) -> (result: Record)`. Per call
//! it makes a fresh realm, installs the prelude (no clock, no randomness, no I/O, no locale),
//! compiles `source` as a function expression, applies it to `args`, and answers `{ value }` or
//! `{ error }`. Nothing survives between calls. Values cross in core's own encoding, so this
//! crate decodes and encodes with `holon-core` itself, the same code the host runs.
//!
//! Determinism, closed at three places: `getrandom` is given a fixed seed (the custom backend
//! below), the prelude removes `Date` and `Math.random`, and `HostHooks::utc_now` answers zero
//! should anything reach for the clock anyway.

use std::collections::BTreeMap;
use std::rc::Rc;

use boa_engine::context::time::FixedClock;
use boa_engine::context::HostHooks;
use boa_engine::object::builtins::JsArray;
use boa_engine::property::PropertyKey;
use boa_engine::{Context, JsObject, JsString, JsValue, Source};
use holon_core::{decode_value, encode_value, Value};

/// What the realm has before the source is compiled: the determinism rule made concrete. Each
/// removal is a conformance test in `holon-module`.
const PRELUDE: &str = r#"
(() => {
  "use strict";
  const g = globalThis;
  for (const name of ["Date", "setTimeout", "setInterval", "clearTimeout", "clearInterval",
                      "queueMicrotask", "fetch", "XMLHttpRequest", "WebAssembly", "Intl", "Temporal"]) {
    delete g[name];
  }
  delete Math.random;
  const tag = (v) => Object.prototype.toString.call(v).slice(8, -1);
  Object.defineProperty(g, "__holon_tag", { value: tag, writable: false, configurable: false, enumerable: false });
})();
"#;

/// `getrandom`'s custom backend: a fixed sequence, so hash seeds and anything else that asks for
/// entropy get the same bytes on every machine. No randomness is available to a body: the
/// prelude removes `Math.random`, and a flow that wants a random number wires `random` in.
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), getrandom::Error> {
    for i in 0..len {
        // SAFETY: the caller hands a buffer of `len` writable bytes.
        unsafe { dest.add(i).write((i as u8).wrapping_mul(31).wrapping_add(7)) };
    }
    Ok(())
}

/// The clock, should anything reach it past the prelude: the epoch, always.
struct Frozen;
impl HostHooks for Frozen {
    fn utc_now(&self) -> i64 {
        0
    }
    fn local_timezone_offset_seconds(&self, _unix_time_seconds: i64) -> i32 {
        0
    }
}

// ── panics, readable ────────────────────────────────────────────────────────────────────────

/// The last panic's message. `panic = "abort"` turns a panic into an `unreachable` trap, which
/// says nothing; the hook below writes the message here first, and `last_panic` hands the host
/// its address, so the engine crate can read it after the trap and name the failure.
static mut LAST_PANIC: [u8; 1024] = [0; 1024];
static mut LAST_PANIC_LEN: usize = 0;

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let text = info.to_string();
            let bytes = text.as_bytes();
            let n = bytes.len().min(1024);
            // SAFETY: single-threaded, and the host reads only after the trap this panic becomes.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), (&raw mut LAST_PANIC) as *mut u8, n);
                LAST_PANIC_LEN = n;
            }
        }));
    });
}

/// The address (high 32 bits) and length (low 32 bits) of the last panic's message; length 0
/// when there was none. Read by the host after a trap.
#[unsafe(no_mangle)]
pub extern "C" fn last_panic() -> i64 {
    // SAFETY: single-threaded; the buffer is only ever written by the hook.
    unsafe {
        let ptr = (&raw const LAST_PANIC) as *const u8 as u32 as u64;
        (ptr << 32) | LAST_PANIC_LEN as u64
    }
    .cast_signed()
}

// ── the ABI ─────────────────────────────────────────────────────────────────────────────────

/// Hands the host `len` writable bytes.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: i32) -> i32 {
    let mut buffer: Vec<u8> = Vec::with_capacity(len.max(1) as usize);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr as i32
}

/// Runs the call encoded at `ptr..ptr+len` and answers the address (high 32 bits) and length (low
/// 32 bits) of its encoded answer.
#[unsafe(no_mangle)]
pub extern "C" fn run(ptr: i32, len: i32) -> i64 {
    // SAFETY: the host wrote `len` bytes at the address `alloc` gave it.
    install_panic_hook();
    let input = unsafe { std::slice::from_raw_parts(ptr as u32 as usize as *const u8, len as u32 as usize) };
    let answer = answer(input);
    let out_len = answer.len() as u64;
    let out_ptr = answer.leak().as_ptr() as u32 as u64;
    ((out_ptr << 32) | out_len) as i64
}

/// The encoded answer to an encoded call: one list holding one record, `{ value }` or `{ error }`.
fn answer(input: &[u8]) -> Vec<u8> {
    let result = match call(input) {
        Ok(Some(v)) => Value::Record(BTreeMap::from([("value".to_string(), v)])),
        Ok(None) => Value::Record(BTreeMap::new()),
        Err(message) => Value::Record(BTreeMap::from([("error".to_string(), Value::Text(message))])),
    };
    encode_value(&Value::List(vec![result])).unwrap_or_default()
}

/// Decode the call, make a realm, compile the source, apply it, convert what it returned.
fn call(input: &[u8]) -> Result<Option<Value>, String> {
    let decoded = decode_value(input).map_err(|e| format!("the call isn't a list of values: {e:?}"))?;
    let Value::List(items) = decoded else { return Err("the call isn't a list".into()) };
    let mut items = items.into_iter();
    let source = match items.next() {
        Some(Value::Text(s)) => s,
        _ => return Err("the first input must be the source, as text".into()),
    };
    let args = match items.next() {
        Some(Value::List(a)) => a,
        Some(other) => vec![other],
        None => vec![],
    };

    // A frozen clock at the epoch: Boa's own would read the system clock, which this target
    // doesn't have, and a body sees no clock anyway (the prelude removes `Date`).
    let mut context = Context::builder()
        .clock(Rc::new(FixedClock::from_millis(0)))
        .host_hooks(Rc::new(Frozen))
        .build()
        .map_err(|e| format!("no realm: {e}"))?;
    context.eval(Source::from_bytes(PRELUDE.as_bytes())).map_err(|e| format!("the prelude failed: {e}"))?;

    let wrapped = format!("({source})");
    let function = context.eval(Source::from_bytes(wrapped.as_bytes())).map_err(|e| e.to_string())?;
    let Some(callable) = function.as_callable() else { return Err("the source isn't a function".into()) };

    let js_args: Vec<JsValue> = args.into_iter().map(|a| to_js(a, &mut context)).collect::<Result<_, _>>()?;
    let returned = callable.call(&JsValue::undefined(), &js_args, &mut context).map_err(|e| e.to_string())?;
    from_js(&returned, &mut context)
}

// ── values across the boundary (docs/L4s/js-node-L4.md §5) ──────────────────────────────────

/// A core value as a JavaScript value. An optional's `none` is `undefined`, `some(x)` is `x`;
/// an effect never enters.
fn to_js(v: Value, context: &mut Context) -> Result<JsValue, String> {
    Ok(match v {
        Value::Number(n) => JsValue::from(n),
        Value::Boolean(b) => JsValue::from(b),
        Value::Text(s) => JsValue::from(JsString::from(s.as_str())),
        Value::List(items) => {
            let elements: Vec<JsValue> = items.into_iter().map(|i| to_js(i, context)).collect::<Result<_, _>>()?;
            JsValue::from(JsObject::from(JsArray::from_iter(elements, context)))
        }
        Value::Record(fields) => {
            let object = JsObject::with_object_proto(context.intrinsics());
            for (k, v) in fields {
                let value = to_js(v, context)?;
                object.set(PropertyKey::from(JsString::from(k.as_str())), value, false, context).map_err(|e| e.to_string())?;
            }
            JsValue::from(object)
        }
        Value::Sum { arm: 1, .. } => JsValue::undefined(),
        Value::Sum { arm: 0, payload } => to_js(*payload, context)?,
        Value::Sum { arm, .. } => return Err(format!("a sum's arm {arm} has no JavaScript form")),
        Value::Effect { .. } => return Err("an effect can't enter a js node".into()),
    })
}

/// A JavaScript value as a core value: `undefined` and `null` are absent, a plain object is a
/// record, an array a list; anything else (a function, a Date, a Map, a Promise, a bigint, a
/// symbol) is an error naming its kind.
fn from_js(v: &JsValue, context: &mut Context) -> Result<Option<Value>, String> {
    if v.is_null_or_undefined() {
        return Ok(None);
    }
    if let Some(b) = v.as_boolean() {
        return Ok(Some(Value::Boolean(b)));
    }
    if let Some(n) = v.as_number() {
        return Ok(Some(Value::Number(n)));
    }
    if let Some(s) = v.as_string() {
        return s.to_std_string().map(|t| Some(Value::Text(t))).map_err(|_| "a text with a lone surrogate can't cross".to_string());
    }
    if v.as_bigint().is_some() {
        return Err(kind_error("bigint"));
    }
    if v.as_symbol().is_some() {
        return Err(kind_error("symbol"));
    }
    let Some(object) = v.as_object() else { return Err(kind_error("value of an unknown kind")) };
    if object.is_callable() {
        return Err(kind_error("function"));
    }
    if object.is_array() {
        let array = JsArray::from_object(object).map_err(|e| e.to_string())?;
        let n = array.length(context).map_err(|e| e.to_string())?;
        let mut items = Vec::with_capacity(n as usize);
        for i in 0..n {
            let element = array.at(i as i64, context).map_err(|e| e.to_string())?;
            items.push(from_js(&element, context)?.unwrap_or_else(none));
        }
        return Ok(Some(Value::List(items)));
    }
    let tag = tag_of(v, context)?;
    if tag != "Object" {
        return Err(kind_error(&tag));
    }
    let mut fields = BTreeMap::new();
    for key in object.own_property_keys(context).map_err(|e| e.to_string())? {
        let name = match &key {
            PropertyKey::String(s) => s.to_std_string().map_err(|_| "a key with a lone surrogate can't cross".to_string())?,
            PropertyKey::Index(i) => i.get().to_string(),
            PropertyKey::Symbol(_) => continue,
        };
        let value = object.get(key, context).map_err(|e| e.to_string())?;
        if let Some(v) = from_js(&value, context)? {
            fields.insert(name, v);
        }
    }
    Ok(Some(Value::Record(fields)))
}

/// `Object.prototype.toString`'s tag for `v`, through the prelude's frozen helper.
fn tag_of(v: &JsValue, context: &mut Context) -> Result<String, String> {
    let helper = context.global_object().get(PropertyKey::from(JsString::from("__holon_tag")), context).map_err(|e| e.to_string())?;
    let Some(f) = helper.as_callable() else { return Err("the prelude's tag helper is gone".into()) };
    let tag = f.call(&JsValue::undefined(), &[v.clone()], context).map_err(|e| e.to_string())?;
    tag.as_string().map(|s| s.to_std_string_lossy()).ok_or_else(|| "the tag isn't text".into())
}

fn kind_error(kind: &str) -> String {
    format!("a js node returned a {kind}; return a number, text, boolean, array or object")
}

/// `none()`, as core spells it.
fn none() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}
