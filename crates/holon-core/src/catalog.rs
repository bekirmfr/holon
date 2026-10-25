//! The authoritative interface of every genesis primitive.
//!
//! Core already maps a primitive *name* to two properties, each in one place: its **behaviour**
//! ([`crate::primitives::compute`]) and its **capability** ([`crate::capabilities_of`]). This is the
//! third axis — a name → its typed **interface** (named input ports, output type, default params) —
//! so the genesis set is defined *by code*, not re-asserted by hand in every test and (next) the
//! editor's palette. Same fail-closed contract: an unknown name yields `None`.
//!
//! The 42 leaves and their grouping are §13a of the design doc (37 original + `to_text`/`is_truthy`/`guard`/`default`/`equals`). The *types* are read off `compute`'s
//! arity/coercions and `eval`'s higher-order handling: variadic leaves (`add`, `and`, `concat`,
//! `list`, …) get their canonical arity 2 (true variadic editing is a later editor concern);
//! polymorphic leaves use [`Type::Any`] (the wiring check already treats `Any` as compatible);
//! `map`/`fold` carry their function as an **open** `fn` param (the editor binds it), matching the
//! evaluator. `interface_of` returns a *default-configured* unit (`const` at `Number 0`, `compare` at
//! `op="="`) so the palette drops a usable, valid node. See primitive-catalog-L4.

use crate::{Param, Port, Type, Unit, UnitBody};

/// Every genesis primitive name (§13a), in manifest order. The enumeration source for the catalog.
pub const GENESIS: &[&str] = &[
    // pure — language core (16)
    "const", "add", "multiply", "subtract", "divide", "compare", "and", "or", "not", "select",
    "cons", "head", "tail", "is_empty", "fold", "map",
    // pure — text (7)
    "concat", "uppercase", "lowercase", "text_length", "slice", "index_of", "split",
    // pure — list convenience, demotion-deferred (7)
    "list", "get", "append", "extend", "length", "sum", "join",
    // pure — counting (1): 0 to n-1, n capped, so a map over it still ends
    "range",
    // pure — product constructor (1)
    "pair",
    // pure — sum constructor (2)
    "some", "none",
    // pure — record kit (9): field names frozen (field, with_field, without_field), or keys that
    // arrive on a wire (lookup, insert, remove), e.g. a route's path param; and find, a list of
    // records searched by one field's value
    "record", "field", "with_field", "without_field", "keys", "lookup", "insert", "remove", "find",
    // pure — JSON (2)
    "json_parse", "json_stringify",
    // pure — signatures and encodings (5): checking a webhook's signature
    "sha256", "hmac_sha256", "hex", "base64", "secure_equals",
    // pure — universal, any-typed (7): as_text is to_text's message form, the identity on text;
    // coalesce is the first input that arrived, `default` with its fallback on a wire
    "to_text", "as_text", "is_truthy", "guard", "default", "coalesce", "equals",
    // request/response — performed by the host, answered in the next round (2): a request, and
    // one whose body is a file a handle names
    "http_request", "http_upload",
    // drivers — sourced, no compute (5): a new file in a folder starts a run, as a request does
    "clock", "random", "ingress", "secret", "file_arrival",
    // durable runs — answered by the host when an event arrives (2)
    "await", "resume_token",
    // state cell (2)
    "store_read", "store_write",
    // collections: keyed items, read by key or in order, put and deleted (4)
    "collection_get", "collection_list", "collection_put", "collection_delete",
    // named folders: files read by name or listed, written and deleted (4); a file's handle, its
    // lines and a slice of it, and a copy by handle (4)
    "file_read", "file_list", "file_write", "file_delete",
    "file_handle", "file_lines", "file_slice", "file_copy",
    // and a write and a copy that never replace a file (2)
    "file_create", "file_create_copy",
    // sink — performed (1)
    "egress",
];

/// Genesis leaves whose `compute` genuinely tolerates any input count — `fold_numbers` /
/// `fold_bools` / an unbounded loop over `inputs` in `primitives.rs` — not the fixed-arity
/// `let [a, b] = inputs else { return None }` pattern most leaves use. `add`/`multiply` fold with
/// identity `0`/`1`; `and`/`or` fold with identity `true`/`false`; `concat`/`list` loop with no
/// count check at all. Every one is well-defined at ANY count, including zero.
///
/// `subtract`/`divide`/`compare` are deliberately excluded even though they share `add`'s catalog
/// line: subtraction and division aren't associative, so "variadic subtract" has no natural
/// meaning core has chosen to give it — this list names only the leaves where growing arity is
/// unambiguous, not "everything binary-shaped."
///
/// `map` and `fold` grow differently: inputs after their list, of any type, are passed to their
/// function after the item (`fn(item, extra…)`, `fn(acc, item, extra…)`), the same on every
/// application. That is how a lambda's captures reach it as inputs (eval's `eval_map`).
///
/// The 4th catalog fact, alongside `compute` / `capabilities_of` / `interface_of`. Purely an
/// editor concern: growing a *placed* node's port count past the catalog's declared default (2,
/// same as every other leaf below) — core neither needs nor enforces this list to evaluate
/// correctly. A `Unit`'s declared `inputs` is free-form data; nothing in `validate`/`hash_unit`
/// checks it against some external "canonical" shape, and these leaves' `compute` (eval, for `map`
/// and `fold`) already handles whatever count of values it's actually given. This constant exists so the editor
/// derives "which leaves may grow" from core, the same way it derives everything else, instead of
/// hand-listing names in TypeScript that could silently drift from what `compute` actually does.
pub const VARIADIC: &[&str] = &["add", "multiply", "and", "or", "concat", "list", "map", "fold"];

/// The variadic leaves whose extra inputs are any type, passed to their function, rather than
/// more of the same port.
pub const PASSES_EXTRA: &[&str] = &["map", "fold"];

/// The canonical genesis interface for `name`, or `None` if it is not a genesis primitive
/// (fail-closed, mirroring [`crate::capabilities_of`]).
pub fn interface_of(name: &str) -> Option<Unit> {
    use Type::*;
    // A primitive unit whose body name is the one being asked for.
    let prim = |inputs: Vec<Port>, params: Vec<Param>, output: Type| Unit {
        body: UnitBody::Primitive { name: name.to_string() },
        params,
        inputs,
        outputs: vec![Port { name: "out".to_string(), ty: output }],
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
            vec![Param::frozen("op", "=")],
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
        // The pieces of `text` between each `sep`: `join`'s inverse.
        "split" => prim(vec![port("text", Text), port("sep", Text)], vec![], Type::List(Box::new(Text))),

        // ── pure — list convenience ──
        "list" => prim(vec![port("a", Any), port("b", Any)], vec![], Type::list_any()),
        "get" => prim(vec![port("list", Type::list_any()), port("index", Number)], vec![], Any),
        "append" => prim(vec![port("list", Type::list_any()), port("item", Any)], vec![], Type::list_any()),
        // `append` adds one item; `extend` adds every item of a second list (`concat` is text's).
        "extend" => prim(vec![port("list", Type::list_any()), port("items", Type::list_any())], vec![], Type::list_any()),
        "length" => prim(vec![port("list", Type::list_any())], vec![], Number),
        "range" => prim(vec![port("n", Number)], vec![], Type::List(Box::new(Number))),
        // sum/join are monomorphic in their element, so they declare it precisely: a typed
        // List<Text> is now refused by sum at wire time (core validate), caught earlier than the
        // old runtime None. A constructed List<Any> still bridges in (the Any wildcard).
        "sum" => prim(vec![port("list", Type::List(Box::new(Number)))], vec![], Number),
        "join" => prim(vec![port("list", Type::List(Box::new(Text))), port("sep", Text)], vec![], Text),

        // ── pure — product ──
        // pair: the binary Product constructor. Output Product([Any, Any]); the value rides the list
        // value, so its compute is list-of-two — only the declared TYPE differs from `list`, and the
        // recursive checker keeps Product and List apart. Dictionary<K,V> is sugar for List<Pair>.
        // ── pure — record kit ── `name` defaults to "id", so a dropped node is valid at once.
        "record" => prim(vec![], vec![], Type::Record),
        "field" => prim(vec![port("record", Type::Record)], vec![Param::frozen("name", "id")], Any),
        "with_field" => prim(
            vec![port("record", Type::Record), port("value", Any)],
            vec![Param::frozen("name", "id")],
            Type::Record,
        ),
        "without_field" => prim(vec![port("record", Type::Record)], vec![Param::frozen("name", "id")], Type::Record),
        "keys" => prim(vec![port("record", Type::Record)], vec![], Type::List(Box::new(Text))),
        // The same three operations with the key as an input: a record used as a keyed collection.
        "lookup" => prim(vec![port("record", Type::Record), port("key", Text)], vec![], Any),
        "insert" => prim(
            vec![port("record", Type::Record), port("key", Text), port("value", Any)],
            vec![],
            Type::Record,
        ),
        "remove" => prim(vec![port("record", Type::Record), port("key", Text)], vec![], Type::Record),
        // The first record in a list whose `name` field equals `value`.
        "find" => prim(vec![port("list", Type::list_any()), port("value", Any)], vec![Param::frozen("name", "id")], Any),
        "json_parse" => prim(vec![port("text", Text)], vec![], Any),
        "json_stringify" => prim(vec![port("value", Any)], vec![], Text),
        "pair" => prim(vec![port("a", Any), port("b", Any)], vec![], Product(vec![Any, Any])),

        // ── pure — signatures and encodings (crypto.rs) ── digests as lowercase `hex` or `base64`.
        "sha256" => prim(vec![port("text", Text)], vec![Param::frozen("encoding", "hex")], Text),
        "hmac_sha256" => prim(vec![port("key", Text), port("message", Text)], vec![Param::frozen("encoding", "hex")], Text),
        "hex" => prim(vec![port("text", Text)], vec![Param::frozen("mode", "encode")], Text),
        "base64" => prim(
            vec![port("text", Text)],
            vec![Param::frozen("mode", "encode"), Param::frozen("alphabet", "standard")],
            Text,
        ),
        "secure_equals" => prim(vec![port("a", Text), port("b", Text)], vec![], Boolean),

        // ── pure — sum (Option) ──
        // Option<T> = Sum([T, Unit]) — arm 0 is "some", arm 1 ("none") is Unit (Product([])),
        // already expressible with no new value kind for THAT half; the new kind is the tag itself
        // ([`crate::Value::Sum`]). some/none are Option-specific, not a generic n-ary injector — the
        // standing example the L4 names; a general inject(tag, value) is deferred until something
        // other than Option asks for it (no constructor needs it yet).
        "some" => prim(vec![port("value", Any)], vec![], Sum(vec![Any, Product(vec![])])),
        "none" => prim(vec![], vec![], Sum(vec![Any, Product(vec![])])),

        // ── pure — universal: accepts every value kind, not a fixed shape ──
        "to_text" | "as_text" => prim(vec![port("value", Any)], vec![], Text),
        "is_truthy" => prim(vec![port("value", Any)], vec![], Boolean),
        "guard" => prim(vec![port("cond", Boolean), port("value", Any)], vec![], Any),
        // `a` when it arrived, else `b`: the fallback on a wire where `default`'s is frozen.
        "coalesce" => prim(vec![port("a", Any), port("b", Any)], vec![], Any),
        "default" => prim(
            vec![port("value", Any)],
            vec![Param::frozen("value", "0"), Param::frozen("type", "Number")],
            Any,
        ),
        "equals" => prim(vec![port("a", Any), port("b", Any)], vec![], Boolean),

        // ── drivers — sourced (no inputs; host-supplied). Concrete where unambiguous, `Any` where
        //    the payload is genuinely polymorphic (the editor may specialize later). ──
        "clock" => prim(vec![], vec![], Number),
        "random" => prim(vec![], vec![], Number),
        "ingress" => prim(vec![], vec![], Any),
        // The handle of the new file that started the run.
        // `on`: `new` (a file new or changed) or `deleted`; `matching`: a glob on its name, empty
        // for every file. The host reads both from the deployed graph.
        "file_arrival" => prim(
            vec![],
            vec![Param::frozen("folder", "files"), Param::frozen("on", "new"), Param::frozen("matching", "")],
            Type::Record,
        ),
        // A named secret from the deployment's secret store (never in the unit: only its name).
        "secret" => prim(vec![], vec![Param::frozen("name", "api_key")], Text),
        // A durable run's waiting point: its event (the request that resumed the run, or
        // `{expired: true}`), absent until it arrives. `expires` is in seconds (7 days).
        "await" => prim(vec![], vec![Param::frozen("name", "event"), Param::frozen("expires", "604800")], Any),
        // The token that delivers an event to the waiting point named `await` (a callback URL's
        // part, say). The same token for the whole run, however often it is evaluated.
        "resume_token" => prim(vec![], vec![Param::frozen("await", "event")], Text),

        // ── state cell ── `"key"` (frozen) names which slot this leaf touches — see the
        // keyed-state-capability L4. `capabilities_of` now fails closed without one, the
        // same standard an unrecognized name already gets held to; the default here
        // ("value") keeps a freshly palette-dropped node immediately valid and usable,
        // matching every other leaf's own default-configured convention (`const` at
        // `Number 0`, `compare` at `op="="`).
        "store_read" => prim(vec![], vec![Param::frozen("key", "value")], Any),
        "store_write" => prim(vec![port("value", Any)], vec![Param::frozen("key", "value")], Effect),

        // ── collections ── `collection` (frozen) names which one; the default ("items") keeps a
        // dropped node valid. A listing's prefix and after are optional inputs; it answers a
        // record of up to `limit` items in key order.
        "collection_get" => prim(vec![port("key", Text)], vec![Param::frozen("collection", "items")], Any),
        "collection_list" => prim(
            vec![port("prefix", Text), port("after", Text)],
            vec![Param::frozen("collection", "items"), Param::frozen("limit", "100")],
            Type::Record,
        ),
        "collection_put" => prim(vec![port("key", Text), port("value", Any)], vec![Param::frozen("collection", "items")], Effect),
        "collection_delete" => prim(vec![port("key", Text)], vec![Param::frozen("collection", "items")], Effect),

        // ── named folders ── `folder` (frozen) names which one; the host binds the name to a real
        // folder. A file is read as text by its name (a path inside the folder); a listing is a
        // record of up to `limit` files in name order, as a collection's.
        "file_read" => prim(vec![port("name", Text)], vec![Param::frozen("folder", "files")], Text),
        "file_list" => prim(
            vec![port("prefix", Text), port("after", Text)],
            vec![Param::frozen("folder", "files"), Param::frozen("limit", "100")],
            Type::Record,
        ),
        "file_write" => prim(vec![port("name", Text), port("text", Text)], vec![Param::frozen("folder", "files")], Effect),
        "file_delete" => prim(vec![port("name", Text)], vec![Param::frozen("folder", "files")], Effect),
        // A file of any size without its bytes: a handle `{folder, name, size, hash, modified}`, to
        // pass on or copy. Part of one: `limit` lines from line `from` (from the end when
        // negative), or `length` bytes from `offset`, as text.
        "file_handle" => prim(vec![port("name", Text)], vec![Param::frozen("folder", "files")], Type::Record),
        "file_lines" => prim(
            vec![port("name", Text), port("from", Number)],
            vec![Param::frozen("folder", "files"), Param::frozen("limit", "100")],
            Type::List(Box::new(Text)),
        ),
        "file_slice" => prim(vec![port("name", Text), port("offset", Number), port("length", Number)], vec![Param::frozen("folder", "files")], Text),
        "file_copy" => prim(vec![port("name", Text), port("from", Type::Record)], vec![Param::frozen("folder", "files")], Effect),
        // As a write and a copy, landing only where no file has the name: never a replace.
        "file_create" => prim(vec![port("name", Text), port("text", Text)], vec![Param::frozen("folder", "files")], Effect),
        "file_create_copy" => prim(vec![port("name", Text), port("from", Type::Record)], vec![Param::frozen("folder", "files")], Effect),

        // ── sink ──
        "egress" => prim(vec![port("payload", Any)], vec![], Effect),
        // `method` and `host` are frozen (the host is the capability); the path, headers and
        // body arrive on wires. The response is `{status, headers, body}`, filled in by the host.
        "http_request" => prim(
            vec![port("path", Text), port("headers", Type::Record), port("body", Any)],
            vec![Param::frozen("method", "GET"), Param::frozen("host", "example.com")],
            Type::Record,
        ),
        // A request whose body is the file a handle names, its bytes streamed by the host.
        "http_upload" => prim(
            vec![port("path", Text), port("headers", Type::Record), port("file", Type::Record)],
            vec![Param::frozen("method", "PUT"), Param::frozen("host", "example.com")],
            Type::Record,
        ),

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
    use crate::{capabilities_of, compatible, eval, Capability, MapResolver, Value};

    #[test]
    fn interface_of_unknown_is_none() {
        // Fail-closed, like capabilities_of: a name the engine doesn't vouch for has no interface.
        assert!(interface_of("while").is_none());
        assert!(interface_of("recurse").is_none());
        assert!(interface_of("").is_none());
    }

    #[test]
    fn genesis_has_79_leaves() {
        // The §13a count — a tripwire if the frozen set is ever changed by accident.
        // 37 + to_text + is_truthy + guard + default + equals (the universal any-typed
        // primitives) + the record kit (record, field, with_field, without_field, keys), added
        // deliberately for M1 (docs/PLATFORM_LAYOUT.md), json_parse + json_stringify, and the
        // keyed record kit (lookup, insert, remove), find, extend, http_request, secret, and the
        // durable-run pair (await, resume_token), the signature kit (sha256, hmac_sha256,
        // hex, base64, secure_equals), split, the collection kit (get, list, put, delete), and
        // as_text (the language's string interpolation) and coalesce (its `??` and `if … else`
        // with a fallback on a wire), both for docs/LANGUAGE.md, range, and the named-folder kit
        // (file_read, file_list, file_write, file_delete, file_handle, file_lines, file_slice,
        // file_copy), and a folder's trigger and a file as a request's body (file_arrival,
        // http_upload), and the create-only pair (file_create, file_create_copy).
        assert_eq!(GENESIS.len(), 83);
    }

    #[test]
    fn coalesce_is_in_the_catalog_with_two_any_inputs() {
        assert!(GENESIS.contains(&"coalesce"));
        let unit = interface_of("coalesce").unwrap();
        let inputs: Vec<_> = unit.inputs.iter().map(|p| (p.name.as_str(), p.ty.clone())).collect();
        assert_eq!(inputs, [("a", Type::Any), ("b", Type::Any)]);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Any);
        assert!(unit.params.is_empty());
        // Not variadic: a variadic leaf fails closed on an absent input, the opposite of this one.
        assert!(!VARIADIC.contains(&"coalesce"));
    }

    #[test]
    fn as_text_is_in_the_catalog_with_to_texts_shape() {
        assert!(GENESIS.contains(&"as_text"));
        let unit = interface_of("as_text").unwrap();
        assert_eq!(unit.inputs.len(), 1);
        assert_eq!(unit.inputs[0].ty, Type::Any);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Text);
        // Its own name in its body, not to_text's (the shared interface arm must not leak it).
        assert_eq!(unit.body, UnitBody::Primitive { name: "as_text".into() });
    }

    #[test]
    fn to_text_is_in_the_catalog_with_the_right_shape() {
        // The palette is DERIVED from GENESIS + interface_of (never a hand-written editor
        // copy, I1) — this is the one test proving the two actually agree for the new
        // leaf, the same way every other genesis primitive already has one.
        assert!(GENESIS.contains(&"to_text"));
        let unit = interface_of("to_text").unwrap();
        assert_eq!(unit.inputs.len(), 1);
        assert_eq!(unit.inputs[0].ty, Type::Any);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Text);
    }

    #[test]
    fn is_truthy_is_in_the_catalog_with_the_right_shape() {
        assert!(GENESIS.contains(&"is_truthy"));
        let unit = interface_of("is_truthy").unwrap();
        assert_eq!(unit.inputs.len(), 1);
        assert_eq!(unit.inputs[0].ty, Type::Any);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Boolean);
    }

    #[test]
    fn guard_is_in_the_catalog_with_the_right_shape() {
        assert!(GENESIS.contains(&"guard"));
        let unit = interface_of("guard").unwrap();
        assert_eq!(unit.inputs.len(), 2);
        assert_eq!(unit.inputs[0].ty, Type::Boolean);
        assert_eq!(unit.inputs[1].ty, Type::Any);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Any);
    }

    #[test]
    fn default_is_in_the_catalog_with_the_right_shape() {
        assert!(GENESIS.contains(&"default"));
        let unit = interface_of("default").unwrap();
        assert_eq!(unit.inputs.len(), 1);
        assert_eq!(unit.inputs[0].ty, Type::Any);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Any);
        // Its default fallback params resolve to a real value on their own, matching
        // const's own default-value convention (a freshly dropped node stays valid).
        assert_eq!(
            crate::primitives::compute("default", &[], &unit.params),
            Some(crate::Value::Number(0.0)),
        );
    }

    #[test]
    fn equals_is_in_the_catalog_with_the_right_shape() {
        assert!(GENESIS.contains(&"equals"));
        let unit = interface_of("equals").unwrap();
        assert_eq!(unit.inputs.len(), 2);
        assert_eq!(unit.inputs[0].ty, Type::Any);
        assert_eq!(unit.inputs[1].ty, Type::Any);
        assert_eq!(unit.outputs.len(), 1);
        assert_eq!(unit.outputs[0].ty, Type::Boolean);
    }

    #[test]
    fn variadic_names_are_genesis_leaves() {
        // The 4th catalog fact must name real leaves — a typo here would silently make the
        // editor offer to grow a port on a primitive that doesn't exist.
        for &name in VARIADIC {
            assert!(GENESIS.contains(&name), "{name} is in VARIADIC but not GENESIS");
        }
    }

    /// Regression: the palette dropped `compare` with `op="eq"`, an operator `compare` doesn't
    /// accept (its ops are `< > <= >= = !=`), so a freshly placed compare node never fired.
    #[test]
    fn compare_default_op_is_one_compare_accepts() {
        use crate::primitives::compute;
        let params = interface_of("compare").unwrap().params;
        let n = |x: f64| Value::Number(x);
        assert_eq!(compute("compare", &[&n(5.0), &n(5.0)], &params), Some(Value::Boolean(true)));
        assert_eq!(compute("compare", &[&n(5.0), &n(6.0)], &params), Some(Value::Boolean(false)));
    }

    #[test]
    fn variadic_leaves_actually_tolerate_more_than_two_inputs() {
        // Not just trusting the classification: prove every VARIADIC name's compute genuinely
        // folds 3 inputs correctly, and that a strictly-binary sibling (subtract, sharing add's
        // catalog line) correctly refuses to, via the arity mismatch in its own destructure.
        use crate::primitives::compute;
        let n = |x: f64| Value::Number(x);
        let b = |x: bool| Value::Boolean(x);
        let t = |x: &str| Value::Text(x.to_string());

        assert_eq!(compute("add", &[&n(1.0), &n(2.0), &n(3.0)], &[]), Some(n(6.0)));
        assert_eq!(compute("multiply", &[&n(2.0), &n(3.0), &n(4.0)], &[]), Some(n(24.0)));
        assert_eq!(compute("and", &[&b(true), &b(true), &b(false)], &[]), Some(b(false)));
        assert_eq!(compute("or", &[&b(false), &b(false), &b(true)], &[]), Some(b(true)));
        assert_eq!(
            compute("concat", &[&t("a"), &t("b"), &t("c")], &[]),
            Some(t("abc"))
        );
        assert_eq!(
            compute("list", &[&n(1.0), &n(2.0), &n(3.0)], &[]),
            Some(Value::List(vec![n(1.0), n(2.0), n(3.0)]))
        );

        // subtract shares add's catalog interface line but is NOT variadic — not associative, so
        // core gives it no N-ary meaning. Its own strict-binary destructure refuses 3 inputs.
        assert!(!VARIADIC.contains(&"subtract"));
        assert_eq!(compute("subtract", &[&n(1.0), &n(2.0), &n(3.0)], &[]), None);
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
        // (a name in one table but not the other) is caught here. Uses each name's REAL
        // catalog params (not bare &[]) so store_read/store_write's own default key
        // actually has to resolve for this to pass — proving the two catalogs agree
        // end to end, not just superficially by name.
        for &n in GENESIS {
            let iface = interface_of(n).unwrap();
            assert!(capabilities_of(n, &iface.params).is_some(), "{n} missing from capabilities_of");
        }
    }

    #[test]
    fn catalogued_units_evaluate() {
        // Prove the interfaces are wired right by *running* them, not just trusting §13a prose:
        // the default const yields 0, and add/not compute as compute() does.
        let r = MapResolver::new();
        let n = |x: f64| Value::Number(x);

        let konst = interface_of("const").unwrap();
        assert_eq!(eval(&konst, &[], &r).into_vec(), vec![Some(n(0.0))]);

        let add = interface_of("add").unwrap();
        assert_eq!(eval(&add, &[Some(n(2.0)), Some(n(3.0))], &r).into_vec(), vec![Some(n(5.0))]);

        let not = interface_of("not").unwrap();
        assert_eq!(eval(&not, &[Some(Value::Boolean(true))], &r).into_vec(), vec![Some(Value::Boolean(false))]);
    }

    #[test]
    fn sum_and_join_are_element_typed() {
        // The monomorphic list leaves declare their element type precisely (P1.3.2 inch 1 follow-on).
        let sum = interface_of("sum").unwrap();
        assert_eq!(sum.inputs[0].ty, Type::List(Box::new(Type::Number)));
        let join = interface_of("join").unwrap();
        assert_eq!(join.inputs[0].ty, Type::List(Box::new(Type::Text)));

        // The tightening bites: a typed List<Text> is refused by sum, but a constructed List<Any>
        // (what the `list` constructor produces) still bridges in — so existing graphs keep working.
        assert!(!compatible(&Type::List(Box::new(Type::Text)), &sum.inputs[0].ty));
        assert!(compatible(&Type::list_any(), &sum.inputs[0].ty));
    }

    #[test]
    fn store_read_write_have_a_valid_default_key() {
        // interface_of's whole point is a usable, valid palette node — confirmed here by
        // actually deriving capabilities from it, not just checking the param exists.
        let read = interface_of("store_read").unwrap();
        let caps = capabilities_of("store_read", &read.params).unwrap();
        assert!(caps.contains(Capability::StateRead("value".to_string())));

        let write = interface_of("store_write").unwrap();
        let caps = capabilities_of("store_write", &write.params).unwrap();
        assert!(caps.contains(Capability::StateWrite("value".to_string())));
    }

    #[test]
    fn pair_declares_a_product() {
        // The binary product constructor declares Product([Any, Any]) — its value rides the list
        // value, but its type is a Product, kept distinct from List by the checker.
        let pair = interface_of("pair").unwrap();
        assert_eq!(pair.inputs.len(), 2);
        assert_eq!(pair.outputs[0].ty, Type::Product(vec![Type::Any, Type::Any]));
    }

    #[test]
    fn some_and_none_declare_an_option() {
        // Both arms of Option<T> declare the SAME Sum shape (arity 2, Unit at slot 1) — the type
        // doesn't change depending on which arm you construct, only the runtime value's tag does.
        let some = interface_of("some").unwrap();
        let none = interface_of("none").unwrap();
        let option_ty = Type::Sum(vec![Type::Any, Type::Product(vec![])]);
        assert_eq!(some.inputs.len(), 1);
        assert_eq!(some.outputs[0].ty, option_ty.clone());
        assert_eq!(none.inputs.len(), 0);
        assert_eq!(none.outputs[0].ty, option_ty);
    }
}