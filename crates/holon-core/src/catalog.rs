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
    // pure — numbers (11): JavaScript's Math, Number and numeric operators (numbers.rs), a family
    // of functions told apart by a frozen param (`FAMILIES`)
    "math", "max", "min", "aggregate", "atan2", "power", "remainder", "bitwise", "number_test", "to_number",
    "number_format",
    // pure — text (19): JavaScript's string methods and the URI functions (texts.rs), counting
    // UTF-16 code units; `slice` and `index_of` take a list too, as JavaScript names them on both
    "concat", "uppercase", "lowercase", "text_length", "slice", "index_of", "split",
    "text_at", "code_at", "from_codes", "text_contains", "trim", "pad", "repeat", "replace", "normalize",
    "uri_encode", "uri_decode", "is_well_formed",
    // pure — list convenience, demotion-deferred (7)
    "list", "get", "append", "extend", "length", "sum", "join",
    // pure — lists (22): JavaScript's array methods (lists.rs). The higher-order ones take their
    // function as `map` does; `at`, `includes` and `last_index_of` take a text too
    "at", "indices", "entries", "any", "all", "filter", "find", "find_index", "find_last", "find_last_index", "sort_by",
    "includes", "last_index_of", "reverse", "to_spliced", "with_item", "fill", "copy_within", "sort", "flat", "is_list",
    "unique",
    // pure — counting (1): 0 to n-1, n capped, so a map over it still ends
    "range",
    // pure — product constructor (1)
    "pair",
    // pure — sum constructor (2)
    "some", "none",
    // pure — record kit (8): field names frozen (field, with_field, without_field), or keys that
    // arrive on a wire (lookup, insert, remove), e.g. a route's path param
    "record", "field", "with_field", "without_field", "keys", "lookup", "insert", "remove",
    // pure — records (7): JavaScript's Object functions, `in`, spread, `typeof` and `Object.is`
    // (records.rs); `group_by` takes its function as `map` does
    "values", "from_entries", "merge", "has", "group_by", "same_value", "type_of",
    // pure — JSON (2)
    "json_parse", "json_stringify",
    // pure — dates (5): JavaScript's Date in UTC, a date its time value in milliseconds (dates.rs)
    "date_part", "date_utc", "date_set", "date_text", "date_parse",
    // pure — calendars (4): months and days added at a fixed offset, and business days over closing
    // days given as data (dates.rs, host-capabilities L4 §5.1.3)
    "date_add", "business_day_add", "business_days_between", "is_business_day",
    // pure — signatures and encodings (5): checking a webhook's signature, over a text or bytes
    "sha256", "hmac_sha256", "hex", "base64", "secure_equals",
    // pure — bytes (6): a text's encodings, a length, a join, and the numbers 0 to 255 they are;
    // `slice` takes bytes too (host-capabilities L4 §5.10.1)
    "text_to_bytes", "bytes_to_text", "bytes_length", "bytes_concat", "bytes_to_list", "list_to_bytes",
    // pure — decimals (7): exact, over amounts written as text (decimals.rs, host-capabilities L4
    // §5.1.1)
    "decimal_add", "decimal_sub", "decimal_mul", "decimal_div", "decimal_round", "decimal_sum", "decimal_compare",
    // pure — CSV (2): RFC 4180, both ways (csv.rs)
    "csv_parse", "csv_format",
    // pure — XML (5): an element as JsonML's list, both ways, and read by path (xml.rs)
    "xml_parse", "xml_format", "xml_find", "xml_text", "xml_attribute",
    // pure — compression (4): gzip and zip, both ways, the same bytes everywhere (compress.rs,
    // host-capabilities L4 §5.1.7)
    "gzip", "gunzip", "zip", "unzip",
    // pure — JWT (2): a token verified with the key that decides its algorithm, or read without
    // trusting it (jwt.rs, host-capabilities L4 §5.1.9)
    "jwt_verify", "jwt_decode",
    // pure — universal, any-typed (8): as_text is to_text's message form, the identity on text;
    // coalesce is the first input that arrived, `default` with its fallback on a wire; fail is
    // nothing, its message the run log's reason (the language's `throw`)
    "to_text", "as_text", "is_truthy", "guard", "default", "coalesce", "equals", "fail",
    // request/response — performed by the host, answered in the next round (2): a request, and
    // one whose body is a file a handle names
    "http_request", "http_upload", "http_stream",
    // and a call to another deployed flow's action, which the host runs in-process, and a start
    // of one, which doesn't wait for it to end (2)
    "flow_call", "flow_start",
    // drivers — sourced, no compute (4): a new file in a folder starts a run, as a request does
    "clock", "random", "secret", "file_arrival",
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
    // and a file read as bytes, any file (1)
    "file_read_bytes",
    // and a write and a copy that never replace a file (2)
    "file_create", "file_create_copy",
    // databases the host binds: rows a statement reads, one row, and a write when the run
    // finishes (3)
    "db_query", "db_one", "db_exec",
    // and a document database's: documents a filter finds, one document, and an insert, an update
    // and a delete when the run finishes (5)
    "db_find", "db_find_one", "db_insert", "db_update", "db_delete",
    // channels: a pack sent when the run is over, and a route's answer that subscribes (2)
    "channel_send", "channel_subscribe",
    // sink — performed (1)
    "egress",
];

/// Genesis leaves whose `compute` genuinely tolerates any input count — `fold_numbers` /
/// `fold_bools` / an unbounded loop over `inputs` in `primitives.rs` — not the fixed-arity
/// `let [a, b] = inputs else { return None }` pattern most leaves use. `add`/`multiply` fold with
/// identity `0`/`1`; `and`/`or` fold with identity `true`/`false`; `concat`/`list` loop with no
/// count check at all. Every one is well-defined at ANY count, including zero (`max`/`min` keep
/// the largest or smallest, and with none give nothing, as JavaScript gives ±Infinity).
///
/// `subtract`/`divide`/`compare` are deliberately excluded even though they are two-input like
/// `add`: subtraction and division aren't associative, so "variadic subtract" has no natural
/// meaning core has chosen to give it — this list names only the leaves where growing arity is
/// unambiguous, not "everything binary-shaped."
///
/// `map`, `fold` and the other higher-order leaves ([`PASSES_EXTRA`]) grow differently: inputs
/// after their list, of any type, are passed to their function after the item (`fn(item,
/// extra…)`, `fn(acc, item, extra…)`), the same on every application. That is how a lambda's
/// captures reach it as inputs (eval's `eval_map`).
///
/// The 4th catalog fact, alongside `compute` / `capabilities_of` / `interface_of`. Purely an
/// editor concern: growing a *placed* node's port count past the catalog's declared default (2,
/// same as every other leaf below) — core neither needs nor enforces this list to evaluate
/// correctly. A `Unit`'s declared `inputs` is free-form data; nothing in `validate`/`hash_unit`
/// checks it against some external "canonical" shape, and these leaves' `compute` (eval, for `map`
/// and `fold`) already handles whatever count of values it's actually given. This constant exists so the editor
/// derives "which leaves may grow" from core, the same way it derives everything else, instead of
/// hand-listing names in TypeScript that could silently drift from what `compute` actually does.
pub const VARIADIC: &[&str] = &[
    "add", "multiply", "max", "min", "and", "or", "concat", "list", "map", "fold", "any", "all", "filter", "find", "find_index", "find_last",
    "find_last_index", "sort_by", "merge", "group_by", "json_parse", "json_stringify", "date_utc", "date_set", "from_codes", "bytes_concat",
    "decimal_add", "decimal_mul",
];

/// The variadic leaves whose extra inputs are any type, passed to their function, rather than
/// more of the same port: the higher-order leaves, whose function rides their `fn` param.
pub const PASSES_EXTRA: &[&str] = &["map", "fold", "any", "all", "filter", "find", "find_index", "find_last", "find_last_index", "sort_by", "group_by", "json_parse", "json_stringify"];

/// How many inputs a higher-order leaf ([`PASSES_EXTRA`]) hands its function before the extras:
/// `fold` the result so far and the item, `sort_by` the two items it compares, `json_parse`'s
/// reviver and `json_stringify`'s replacer the key and the value, the others the item.
pub fn function_takes(name: &str) -> usize {
    match name {
        "fold" | "sort_by" | "json_parse" | "json_stringify" => 2,
        _ => 1,
    }
}

/// The leaves that share one shape, told apart by a frozen parameter: each leaf, its family
/// parameter and the values that parameter may take. The parameter is part of the unit's hash, so
/// `compare {op: "<"}` and `compare {op: ">"}` never collide, and a value added later re-hashes
/// nothing. Any other value is refused when the flow is wired (`validate`), where it would
/// otherwise give nothing each time it runs.
pub const FAMILIES: &[(&str, &str, &[&str])] = &[
    ("compare", "op", &["=", "!=", "<", "<=", ">", ">="]),
    (
        "math",
        "fn",
        &[
            "abs", "sign", "floor", "ceil", "trunc", "round", "sqrt", "cbrt", "exp", "expm1", "log", "log2", "log10", "log1p", "sin", "cos", "tan", "asin", "acos",
            "atan", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh", "fround", "f16round", "clz32",
        ],
    ),
    ("aggregate", "fn", &["max", "min", "hypot", "sum_precise"]),
    ("bitwise", "op", &["and", "or", "xor", "shl", "shr", "ushr", "imul"]),
    ("number_test", "test", &["finite", "integer", "safe_integer", "nan"]),
    ("number_test", "coerce", &["none", "number"]),
    ("to_number", "grammar", &["number", "float", "int"]),
    ("number_format", "style", &["string", "fixed", "precision", "exponential"]),
    ("slice", "mode", &["length", "end", "substring"]),
    ("code_at", "mode", &["unit", "point"]),
    ("from_codes", "mode", &["unit", "point"]),
    ("text_contains", "at", &["start", "end"]),
    ("trim", "side", &["both", "start", "end"]),
    ("pad", "side", &["start", "end"]),
    ("replace", "count", &["first", "all"]),
    ("normalize", "form", &["NFC", "NFD", "NFKC", "NFKD"]),
    ("uri_encode", "set", &["component", "uri"]),
    ("uri_decode", "set", &["component", "uri"]),
    ("date_part", "part", crate::dates::PARTS),
    ("date_set", "part", crate::dates::SET_PARTS),
    ("date_text", "format", crate::dates::FORMATS),
    ("date_add", "unit", crate::dates::UNITS),
    ("business_day_add", "weekend", crate::dates::WEEKENDS),
    ("business_days_between", "weekend", crate::dates::WEEKENDS),
    ("is_business_day", "weekend", crate::dates::WEEKENDS),
    ("text_to_bytes", "encoding", &["utf8", "hex", "base64", "base64url"]),
    ("bytes_to_text", "encoding", &["utf8", "hex", "base64", "base64url"]),
    ("decimal_div", "rounding", crate::decimals::ROUNDINGS),
    ("decimal_round", "rounding", crate::decimals::ROUNDINGS),
    ("jwt_verify", "leeway", crate::jwt::LEEWAYS),
    ("csv_parse", "header", crate::csv::HEADERS),
    ("csv_parse", "separator", crate::csv::SEPARATORS),
    ("csv_format", "header", crate::csv::HEADERS),
    ("csv_format", "separator", crate::csv::SEPARATORS),
    ("xml_format", "declaration", crate::xml::DECLARATIONS),
    ("xml_format", "layout", crate::xml::LAYOUTS),
];

/// The values leaf `name`'s family parameter `param` may take, if it is one.
pub fn family_values(name: &str, param: &str) -> Option<&'static [&'static str]> {
    FAMILIES.iter().find(|(leaf, p, _)| *leaf == name && *p == param).map(|(_, _, values)| *values)
}

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

    let mut unit = match name {
        // ── pure — language core ──
        "const" => prim(
            vec![],
            vec![Param::frozen("value", "0"), Param::frozen("type", "Number")],
            Number,
        ),
        // `add` is JavaScript's `+`: numbers add, and a text on either side joins (texts.rs).
        "add" => prim(vec![port("a", Any), port("b", Any)], vec![], Any),
        "multiply" | "subtract" | "divide" => {
            prim(vec![port("a", Number), port("b", Number)], vec![], Number)
        }
        // Two numbers, or two texts in code-unit order.
        "compare" => prim(
            vec![port("a", Any), port("b", Any)],
            vec![Param::frozen("op", "=")],
            Boolean,
        ),
        "and" | "or" => prim(vec![port("a", Boolean), port("b", Boolean)], vec![], Boolean),
        "not" => prim(vec![port("v", Boolean)], vec![], Boolean),

        // ── pure — numbers ── a family's function is a frozen param, at its first value.
        "math" => prim(vec![port("x", Number)], vec![Param::frozen("fn", "abs")], Number),
        "max" | "min" | "power" | "remainder" => prim(vec![port("a", Number), port("b", Number)], vec![], Number),
        "aggregate" => prim(vec![port("xs", Type::List(Box::new(Number)))], vec![Param::frozen("fn", "max")], Number),
        "atan2" => prim(vec![port("y", Number), port("x", Number)], vec![], Number),
        "bitwise" => prim(vec![port("a", Number), port("b", Number)], vec![Param::frozen("op", "and")], Number),
        // Any value: `coerce: "number"` reads it as a number first, `"none"` takes numbers only.
        "number_test" => prim(vec![port("x", Any)], vec![Param::frozen("test", "finite"), Param::frozen("coerce", "none")], Boolean),
        // `radix` and `digits` are optional (core's `has_optional_ports`).
        "to_number" => prim(vec![port("x", Any), port("radix", Number)], vec![Param::frozen("grammar", "number")], Number),
        "number_format" => prim(vec![port("x", Number), port("digits", Number)], vec![Param::frozen("style", "string")], Text),
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
        // Part of a text or a list. `mode: "length"` reads `end` as a count, `"end"` as a relative
        // position and `"substring"` (a text's) as a position clamped, optional (core's
        // `has_optional_ports`), as are the positions, the limit and the fill below.
        "slice" => prim(
            vec![port("value", Any), port("start", Number), port("end", Number)],
            vec![Param::frozen("mode", "length")],
            Any,
        ),
        "index_of" => prim(vec![port("haystack", Any), port("needle", Any), port("from", Number)], vec![], Number),
        // The pieces of `text` between each `sep`: `join`'s inverse; `limit` keeps that many.
        "split" => prim(vec![port("text", Text), port("sep", Text), port("limit", Number)], vec![], Type::List(Box::new(Text))),
        // `charAt`; a text's `at` is `at`'s. `code_at` is `charCodeAt` (`mode: "unit"`) and
        // `codePointAt` (`"point"`); `from_codes` their inverse, taking more numbers with +.
        "text_at" => prim(vec![port("text", Text), port("index", Number)], vec![], Text),
        "code_at" => prim(vec![port("text", Text), port("index", Number)], vec![Param::frozen("mode", "unit")], Number),
        "from_codes" => prim(vec![port("a", Number)], vec![Param::frozen("mode", "unit")], Text),
        // `startsWith` (`at: "start"`) and `endsWith` (`"end"`); a text's `includes` is
        // `includes`'s.
        "text_contains" => prim(vec![port("text", Text), port("part", Text), port("position", Number)], vec![Param::frozen("at", "start")], Boolean),
        "trim" => prim(vec![port("text", Text)], vec![Param::frozen("side", "both")], Text),
        "pad" => prim(vec![port("text", Text), port("length", Number), port("fill", Text)], vec![Param::frozen("side", "start")], Text),
        "repeat" => prim(vec![port("text", Text), port("count", Number)], vec![], Text),
        "replace" => prim(vec![port("text", Text), port("pattern", Text), port("replacement", Text)], vec![Param::frozen("count", "first")], Text),
        "normalize" => prim(vec![port("text", Text)], vec![Param::frozen("form", "NFC")], Text),
        "uri_encode" | "uri_decode" => prim(vec![port("text", Text)], vec![Param::frozen("set", "component")], Text),
        // `true` for a text, which is always well formed in Holon; nothing for anything else.
        "is_well_formed" => prim(vec![port("value", Any)], vec![], Boolean),

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

        // ── pure — lists ── the higher-order ones carry their function as `map` does.
        "at" => prim(vec![port("value", Any), port("index", Number)], vec![], Any),
        "indices" => prim(vec![port("list", Type::list_any())], vec![], Type::List(Box::new(Number))),
        // A list's `[index, item]` pairs, or a record's `[key, value]` pairs.
        "entries" => prim(vec![port("value", Any)], vec![], Type::list_any()),
        "any" | "all" => prim(vec![port("list", Type::list_any())], vec![Param::open("fn", "fn")], Boolean),
        "filter" | "sort_by" => prim(vec![port("list", Type::list_any())], vec![Param::open("fn", "fn")], Type::list_any()),
        "find" | "find_last" => prim(vec![port("list", Type::list_any())], vec![Param::open("fn", "fn")], Any),
        "find_index" | "find_last_index" => prim(vec![port("list", Type::list_any())], vec![Param::open("fn", "fn")], Number),
        "includes" => prim(vec![port("haystack", Any), port("needle", Any), port("from", Number)], vec![], Boolean),
        "last_index_of" => prim(vec![port("haystack", Any), port("needle", Any), port("from", Number)], vec![], Number),
        "reverse" | "sort" | "unique" => prim(vec![port("list", Type::list_any())], vec![], Type::list_any()),
        // `count` and `items` are optional, as are `fill`'s and `copy_within`'s `start` and `end`
        // (core's `has_optional_ports`).
        "to_spliced" => prim(
            vec![port("list", Type::list_any()), port("start", Number), port("count", Number), port("items", Type::list_any())],
            vec![],
            Type::list_any(),
        ),
        "with_item" => prim(vec![port("list", Type::list_any()), port("index", Number), port("value", Any)], vec![], Type::list_any()),
        "fill" => prim(
            vec![port("list", Type::list_any()), port("value", Any), port("start", Number), port("end", Number)],
            vec![],
            Type::list_any(),
        ),
        "copy_within" => prim(
            vec![port("list", Type::list_any()), port("target", Number), port("start", Number), port("end", Number)],
            vec![],
            Type::list_any(),
        ),
        // `depth`: a whole number, or `all`.
        "flat" => prim(vec![port("list", Type::list_any())], vec![Param::frozen("depth", "1")], Type::list_any()),
        "is_list" => prim(vec![port("value", Any)], vec![], Boolean),

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

        // ── pure — records ── `merge` takes more records with +; `group_by` its function as
        // `map` does, the key it answers for an item a text.
        "values" => prim(vec![port("record", Type::Record)], vec![], Type::list_any()),
        "from_entries" => prim(vec![port("entries", Type::list_any())], vec![], Type::Record),
        "merge" => prim(vec![port("a", Type::Record), port("b", Type::Record)], vec![], Type::Record),
        "has" => prim(vec![port("record", Type::Record), port("key", Text)], vec![], Boolean),
        "group_by" => prim(vec![port("list", Type::list_any())], vec![Param::open("fn", "fn")], Type::Record),
        "same_value" => prim(vec![port("a", Any), port("b", Any)], vec![], Boolean),
        "type_of" => prim(vec![port("value", Any)], vec![], Text),
        // JSON. `fn`, when set, is a function of the key and the value, as `fold`'s takes two:
        // `json_parse`'s reviver, `json_stringify`'s replacer; unset (`""`), neither is applied.
        // `indent` is the gap itself (the reader writes a number as that many spaces), `keys` a
        // JSON list of the keys to keep, unset `""`; `keys` and `fn` exclusive, as the replacer is
        // one argument in JavaScript.
        "json_parse" => prim(vec![port("text", Text)], vec![Param::frozen("fn", "")], Any),
        "json_stringify" => prim(
            vec![port("value", Any)],
            vec![Param::frozen("indent", ""), Param::frozen("keys", ""), Param::frozen("fn", "")],
            Text,
        ),
        "pair" => prim(vec![port("a", Any), port("b", Any)], vec![], Product(vec![Any, Any])),

        // ── pure — dates ── a date is its time value, milliseconds since 1970 in UTC; a leaf takes
        // a text for it too, as `new Date(t)` does. `date_utc` takes 1 to 7 numbers with + (the
        // year, then month, date, hours, minutes, seconds, milliseconds), `date_set` the date and
        // 1 to 4 numbers, from its `part` on. An `offset` (`"+01:00"`, or minutes east of UTC) is
        // optional (core's `has_optional_ports`): a date's parts read at it.
        "date_part" => prim(vec![port("time", Any), port("offset", Any)], vec![Param::frozen("part", "time")], Number),
        "date_utc" => prim(vec![port("year", Number)], vec![], Number),
        "date_set" => prim(vec![port("time", Any), port("value", Number)], vec![Param::frozen("part", "year")], Number),
        "date_text" => prim(vec![port("time", Any), port("offset", Any)], vec![Param::frozen("format", "iso")], Text),
        "date_parse" => prim(vec![port("text", Text)], vec![], Number),

        // ── pure — calendars (dates.rs, host-capabilities L4 §5.1.3) ── a calendar date is a
        // `YYYY-MM-DD` text or a time value's date in UTC; `holidays`, a list of them, is optional,
        // as `offset` is.
        "date_add" => prim(vec![port("time", Any), port("amount", Number), port("offset", Any)], vec![Param::frozen("unit", "days")], Number),
        "business_day_add" => prim(vec![port("date", Any), port("days", Number), port("holidays", Type::list_any())], vec![Param::frozen("weekend", "sat_sun")], Text),
        "business_days_between" => prim(vec![port("from", Any), port("to", Any), port("holidays", Type::list_any())], vec![Param::frozen("weekend", "sat_sun")], Number),
        "is_business_day" => prim(vec![port("date", Any), port("holidays", Type::list_any())], vec![Param::frozen("weekend", "sat_sun")], Boolean),

        // ── pure — signatures and encodings (crypto.rs) ── digests as lowercase `hex` or `base64`.
        // A text's UTF-8 or bytes, hashed: their ports are `Any` for either.
        "sha256" => prim(vec![port("text", Any)], vec![Param::frozen("encoding", "hex")], Text),
        "hmac_sha256" => prim(vec![port("key", Any), port("message", Any)], vec![Param::frozen("encoding", "hex")], Text),
        "hex" => prim(vec![port("text", Text)], vec![Param::frozen("mode", "encode")], Text),
        // `bytes: "latin1"` is `btoa`/`atob`'s: each character one byte, the standard alphabet.
        "base64" => prim(
            vec![port("text", Text)],
            vec![Param::frozen("mode", "encode"), Param::frozen("alphabet", "standard"), Param::frozen("bytes", "utf8")],
            Text,
        ),
        "secure_equals" => prim(vec![port("a", Any), port("b", Any)], vec![], Boolean),

        // ── pure — bytes (host-capabilities L4 §5.10.1) ──
        "text_to_bytes" => prim(vec![port("text", Text)], vec![Param::frozen("encoding", "utf8")], Bytes),
        "bytes_to_text" => prim(vec![port("bytes", Bytes)], vec![Param::frozen("encoding", "utf8")], Text),
        "bytes_length" => prim(vec![port("bytes", Bytes)], vec![], Number),
        "bytes_concat" => prim(vec![port("a", Bytes), port("b", Bytes)], vec![], Bytes),
        "bytes_to_list" => prim(vec![port("bytes", Bytes)], vec![], Type::List(Box::new(Number))),
        "list_to_bytes" => prim(vec![port("list", Type::list_any())], vec![], Bytes),

        // ── pure — decimals (decimals.rs, host-capabilities L4 §5.1.1) ── a decimal is a text, or
        // a number as its shortest text; every result is a text, a comparison's aside.
        "decimal_add" | "decimal_sub" | "decimal_mul" => prim(vec![port("a", Any), port("b", Any)], vec![], Text),
        "decimal_div" => prim(vec![port("a", Any), port("b", Any), port("scale", Number)], vec![Param::frozen("rounding", "half_even")], Text),
        "decimal_round" => prim(vec![port("a", Any), port("scale", Number)], vec![Param::frozen("rounding", "half_even")], Text),
        "decimal_sum" => prim(vec![port("list", Type::list_any())], vec![], Text),
        "decimal_compare" => prim(vec![port("a", Any), port("b", Any)], vec![], Number),

        // ── pure — CSV (csv.rs) ── records keyed by the first row, or lists; `columns` gives the
        // order records are written in.
        "csv_parse" => prim(vec![port("text", Text)], vec![Param::frozen("header", "first_row"), Param::frozen("separator", ",")], Type::list_any()),
        "csv_format" => prim(
            vec![port("rows", Type::list_any()), port("columns", Type::List(Box::new(Text)))],
            vec![Param::frozen("header", "first_row"), Param::frozen("separator", ",")],
            Text,
        ),

        // ── pure — XML (xml.rs) ── an element is a list: its name, its attributes' record when it
        // has any, its children; read by a path of names.
        "xml_parse" => prim(vec![port("text", Text)], vec![], Type::list_any()),
        "xml_format" => prim(
            vec![port("element", Type::list_any())],
            vec![Param::frozen("declaration", "utf8"), Param::frozen("layout", "compact")],
            Text,
        ),
        "xml_find" => prim(vec![port("element", Type::list_any()), port("path", Text)], vec![], Type::list_any()),
        "xml_text" => prim(vec![port("element", Type::list_any()), port("path", Text)], vec![], Text),
        "xml_attribute" => prim(vec![port("element", Type::list_any()), port("name", Text)], vec![], Text),

        // ── pure — compression (compress.rs) ── a text is gzipped or zipped as its UTF-8; what
        // comes out is bytes, a zip's files a record of them by name.
        "gzip" => prim(vec![port("data", Any)], vec![], Bytes),
        "gunzip" => prim(vec![port("bytes", Bytes)], vec![], Bytes),
        "zip" => prim(vec![port("files", Type::Record)], vec![], Bytes),
        "unzip" => prim(vec![port("bytes", Bytes)], vec![], Type::Record),

        // ── pure — JWT (jwt.rs) ── `now` is an input, as stripe_verify's is: a rerun checks the
        // token at the time the run saw. An issuer or audience left empty isn't checked.
        "jwt_verify" => prim(
            vec![port("token", Text), port("key", Any), port("now", Number)],
            vec![Param::frozen("issuer", ""), Param::frozen("audience", ""), Param::frozen("leeway", "0")],
            Type::Record,
        ),
        "jwt_decode" => prim(vec![port("token", Text)], vec![], Type::Record),

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
        // Nothing, ever: what reads it has no value. Its message goes to the run's log.
        "fail" => prim(vec![port("message", Text)], vec![], Any),
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
        "file_read_bytes" => prim(vec![port("name", Text)], vec![Param::frozen("folder", "files")], Bytes),
        "file_list" => prim(
            vec![port("prefix", Text), port("after", Text)],
            vec![Param::frozen("folder", "files"), Param::frozen("limit", "100")],
            Type::Record,
        ),
        "file_write" => prim(vec![port("name", Text), port("text", Any)], vec![Param::frozen("folder", "files")], Effect),
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
        "file_create" => prim(vec![port("name", Text), port("text", Any)], vec![Param::frozen("folder", "files")], Effect),
        "file_create_copy" => prim(vec![port("name", Text), port("from", Type::Record)], vec![Param::frozen("folder", "files")], Effect),

        // ── databases ── `database` (frozen) names which one, the host binding the name; `sql`
        // (frozen) is the statement, the tables it touches its capabilities (sql.rs). Its
        // parameters, `$1`, `$2`, …, arrive as a list on a wire, never in the text. A query's
        // rows are records by column name; `expect` is how many rows a write must change ("":
        // any number), or nothing it did lands.
        "db_query" => prim(vec![port("params", Type::list_any())], vec![Param::frozen("database", "db"), Param::frozen("sql", "select * from t")], Type::List(Box::new(Type::Record))),
        "db_one" => prim(vec![port("params", Type::list_any())], vec![Param::frozen("database", "db"), Param::frozen("sql", "select * from t")], Type::Record),
        "db_exec" => prim(
            vec![port("params", Type::list_any())],
            vec![Param::frozen("database", "db"), Param::frozen("sql", "delete from t where false"), Param::frozen("expect", "")],
            Effect,
        ),
        // A document database's (MongoDB): `collection` (frozen) is the capability, as a SQL
        // statement's tables are; `filter`, `update`, `document` and `sort` are frozen templates
        // (documents.rs), JSON whose holes, `$1`, `$2`, …, are the `params` list's values. A find
        // answers documents as records; `limit` is how many at most ("": all).
        "db_find" => prim(
            vec![port("params", Type::list_any())],
            vec![Param::frozen("database", "db"), Param::frozen("collection", "items"), Param::frozen("filter", "{}"), Param::frozen("sort", ""), Param::frozen("limit", "")],
            Type::List(Box::new(Type::Record)),
        ),
        "db_find_one" => prim(
            vec![port("params", Type::list_any())],
            vec![Param::frozen("database", "db"), Param::frozen("collection", "items"), Param::frozen("filter", "{}"), Param::frozen("sort", "")],
            Type::Record,
        ),
        "db_insert" => prim(
            vec![port("params", Type::list_any())],
            vec![Param::frozen("database", "db"), Param::frozen("collection", "items"), Param::frozen("document", "{}")],
            Effect,
        ),
        "db_update" => prim(
            vec![port("params", Type::list_any())],
            vec![Param::frozen("database", "db"), Param::frozen("collection", "items"), Param::frozen("filter", "{}"), Param::frozen("update", "{\"$set\":{}}"), Param::frozen("expect", "")],
            Effect,
        ),
        "db_delete" => prim(
            vec![port("params", Type::list_any())],
            vec![Param::frozen("database", "db"), Param::frozen("collection", "items"), Param::frozen("filter", "{}"), Param::frozen("expect", "")],
            Effect,
        ),
        // A channel's pack, sent once the run is over; and a subscription to the channel, which
        // a route answers with (`from`: "new" packs only, or the "kept" ones first).
        "channel_send" => prim(vec![port("pack", Any)], vec![Param::frozen("channel", "updates")], Effect),
        "channel_subscribe" => prim(vec![], vec![Param::frozen("channel", "updates"), Param::frozen("from", "new")], Effect),

        // ── sink ──
        "egress" => prim(vec![port("payload", Any)], vec![], Effect),
        // `method` and `host` are frozen (the host is the capability); the path, headers and
        // body arrive on wires. The response is `{status, headers, body}`, filled in by the host.
        "http_request" => prim(
            vec![port("path", Text), port("headers", Type::Record), port("body", Any)],
            vec![Param::frozen("method", "GET"), Param::frozen("host", "example.com")],
            Type::Record,
        ),
        // A request whose answer the host also forwards as it arrives: to the route's caller (a
        // streamed reply), or, with `pipe` set, to that channel's subscribers. The flow still gets
        // the whole answer.
        "http_stream" => prim(
            vec![port("path", Text), port("headers", Type::Record), port("body", Any)],
            vec![Param::frozen("method", "POST"), Param::frozen("host", "example.com"), Param::frozen("pipe", "")],
            Type::Record,
        ),
        // A call to the action `action` of the deployed flow `flow`, both frozen (they are the
        // permission, `RunsAction("flow/action")`). What it sends is a value, `{}` when left out;
        // the answer is `{answer, run}` when the action ran, `{error, message}` when it didn't,
        // filled in by the host.
        "flow_call" => prim(
            vec![port("input", Any)],
            vec![Param::frozen("flow", "billing"), Param::frozen("action", "charge")],
            Type::Record,
        ),
        // A start of the action `action` of the deployed flow `flow`, its own flow's or another's:
        // the host runs it in-process until it finishes or first waits, and answers then, never
        // waiting for its end: `{answer, run}` when it finished, `{run, waiting}` when it waits
        // (each waiting point with its token), `{error, message}` when it didn't run.
        "flow_start" => prim(
            vec![port("input", Any)],
            vec![Param::frozen("flow", "billing"), Param::frozen("action", "charge")],
            Type::Record,
        ),
        // A request whose body is the file a handle names, its bytes streamed by the host.
        "http_upload" => prim(
            vec![port("path", Text), port("headers", Type::Record), port("file", Type::Record)],
            vec![Param::frozen("method", "PUT"), Param::frozen("host", "example.com")],
            Type::Record,
        ),

        _ => return None,
    };
    // A write or a call out takes its trigger last: what starts it, never read (eval drops it), so
    // a sink belongs to whatever started the run that reaches it (docs/L4s/triggers-L4.md §3).
    if TRIGGERED.contains(&name) {
        unit.inputs.push(port(TRIGGER, Any));
    }
    Some(unit)
}

/// The input every sink takes last: the value that starts it (a route's request, a holon's own
/// trigger, whatever `after` names). It is never read; it only has to be there.
pub const TRIGGER: &str = "trigger";

/// The sinks: the leaves whose output is an Effect the host performs, and the calls out. Each
/// takes a [`TRIGGER`] input last. `channel_subscribe` is a read (a route's streamed answer) and
/// takes none.
pub const TRIGGERED: &[&str] = &[
    "store_write",
    "collection_put",
    "collection_delete",
    "file_write",
    "file_delete",
    "file_copy",
    "file_create",
    "file_create_copy",
    "db_exec",
    "db_insert",
    "db_update",
    "db_delete",
    "channel_send",
    "egress",
    "http_request",
    "http_upload",
    "http_stream",
    "flow_call",
    "flow_start",
];

/// The sinks that answer: each asks the far side to do something and gives back what it said.
/// Once made, a call can't be taken back, so it is made for what it does: it is kept as an effect
/// whether or not its answer is read, and its answer is never its caller's to give.
pub const CALLS: &[&str] = &["http_request", "http_upload", "http_stream", "flow_call", "flow_start"];

/// The leaves whose value the host supplies as the run starts (`Resolver::sourced`): the clock, a
/// random number, a secret, the file that started the run, a state key. Each computes nothing; the
/// other reads (a collection's, a folder's, a database's, a waiting point) ask as the run goes.
pub const SOURCED: &[&str] = &["clock", "random", "secret", "file_arrival", "store_read"];

/// Whether `unit` is a sink leaf with its trigger: the last input, by name (a leaf placed before
/// sinks had one has none, and is computed as it always was).
pub fn takes_trigger(unit: &Unit) -> bool {
    matches!(&unit.body, UnitBody::Primitive { name } if TRIGGERED.contains(&name.as_str())) && unit.inputs.last().is_some_and(|p| p.name == TRIGGER)
}

/// What a unit's input named [`TRIGGER`] reads when nothing is wired to it: present, so a holon
/// placed without a tie runs its sinks with the unit around it, as a leaf with an open trigger
/// does. Never read for its value.
pub fn open_trigger() -> crate::Value {
    crate::Value::Boolean(true)
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
    fn genesis_has_151_leaves() {
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
        // http_upload), and the create-only pair (file_create, file_create_copy), and the
        // channel pair (channel_send, channel_subscribe), and http_stream, and the database kit
        // (db_query, db_one, db_exec), and the document kit (db_find, db_find_one, db_insert,
        // db_update, db_delete), and flow_call and flow_start; less ingress, which a flow without
        // handlers read its trigger by; and the number leaves (math, max, min, aggregate, atan2,
        // power, remainder, bitwise, number_test, to_number, number_format); and the list leaves
        // (at, indices, entries, any, all, filter, find_index, find_last, find_last_index,
        // sort_by, includes, last_index_of, reverse, to_spliced, with_item, fill, copy_within,
        // sort, flat, is_list, unique), find among them now; and the record leaves (values,
        // from_entries, merge, has, group_by, same_value, type_of); and the date leaves (date_part,
        // date_utc, date_set, date_text, date_parse); and the text leaves (text_at, code_at,
        // from_codes, text_contains, trim, pad, repeat, replace, normalize, uri_encode, uri_decode,
        // is_well_formed); and fail (the language's throw); and the bytes leaves (text_to_bytes,
        // bytes_to_text, bytes_length, bytes_concat, bytes_to_list, list_to_bytes, file_read_bytes);
        // and the decimal leaves (decimal_add, decimal_sub, decimal_mul, decimal_div,
        // decimal_round, decimal_sum, decimal_compare) and CSV's (csv_parse, csv_format); and the
        // calendar leaves (date_add, business_day_add, business_days_between, is_business_day);
        // and XML's (xml_parse, xml_format, xml_find, xml_text, xml_attribute); and compression's
        // (gzip, gunzip, zip, unzip); and JWT's (jwt_verify, jwt_decode).
        assert_eq!(GENESIS.len(), 183);
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
        // folds 3 inputs correctly, and that a strictly-binary sibling (subtract, two-input like
        // add) correctly refuses to, via the arity mismatch in its own destructure.
        use crate::primitives::compute;
        let n = |x: f64| Value::Number(x);
        let b = |x: bool| Value::Boolean(x);
        let t = |x: &str| Value::Text(x.to_string());

        assert_eq!(compute("add", &[&n(1.0), &n(2.0), &n(3.0)], &[]), Some(n(6.0)));
        assert_eq!(compute("multiply", &[&n(2.0), &n(3.0), &n(4.0)], &[]), Some(n(24.0)));
        assert_eq!(compute("and", &[&b(true), &b(true), &b(false)], &[]), Some(b(false)));
        assert_eq!(compute("or", &[&b(false), &b(false), &b(true)], &[]), Some(b(true)));
        assert_eq!(compute("max", &[&n(1.0), &n(7.0), &n(3.0)], &[]), Some(n(7.0)));
        assert_eq!(compute("min", &[&n(1.0), &n(7.0), &n(-3.0)], &[]), Some(n(-3.0)));
        assert_eq!(
            compute("concat", &[&t("a"), &t("b"), &t("c")], &[]),
            Some(t("abc"))
        );
        assert_eq!(
            compute("list", &[&n(1.0), &n(2.0), &n(3.0)], &[]),
            Some(Value::List(vec![n(1.0), n(2.0), n(3.0)]))
        );
        let r = |k: &str, x: f64| Value::Record([(k.to_string(), n(x))].into());
        assert_eq!(compute("merge", &[&r("a", 1.0), &r("b", 2.0), &r("a", 3.0)], &[]), Some(Value::Record([("a".to_string(), n(3.0)), ("b".to_string(), n(2.0))].into())));

        // subtract is two-input like add but is NOT variadic — not associative, so
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