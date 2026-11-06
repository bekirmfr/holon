//! What each genesis primitive does, in a sentence or two, for the people placing it: the fifth
//! catalog fact, keyed by name like the others (behaviour in [`crate::primitives::compute`],
//! capability in [`crate::capabilities_of`], interface in [`crate::interface_of`], variadic in
//! [`crate::VARIADIC`]). The editor shows it on each node and in the palette, read from here so it
//! can't drift from core: a test holds every genesis name to having one.
//!
//! Every node shares one rule, said once here rather than on each: a node whose wired input
//! didn't arrive produces nothing, and nothing flows on from it (`default` is how a flow fills a
//! gap). "Produces nothing" below means the same.

/// What `name` does, or `None` if it is not a genesis primitive.
pub fn describe(name: &str) -> Option<&'static str> {
    Some(match name {
        // ── language core ──
        "const" => "A fixed value set on the node: a number, true or false, text, or JSON. The type picks which.",
        "add" => "Adds its numbers. Takes more inputs with +.",
        "multiply" => "Multiplies its numbers. Takes more inputs with +.",
        "subtract" => "Subtracts b from a: a − b.",
        "divide" => "Divides a by b: a ÷ b. Dividing by zero produces nothing.",
        "compare" => "Compares two numbers with the chosen operator (=, !=, <, <=, >, >=): true or false.",
        "and" => "True when every input is true. Takes more inputs with +.",
        "or" => "True when any input is true. Takes more inputs with +.",
        "not" => "The opposite of a true or false.",
        "select" => "Passes on if_true when cond is true, if_false when it's false.",
        "cons" => "A list with item put first, before the items of list.",
        "head" => "A list's first item; nothing for an empty list.",
        "tail" => "A list without its first item; nothing for an empty list.",
        "is_empty" => "Whether a list has no items.",
        "fold" => "Runs its function over a list, carrying a result along: starting from init, each item and the result so far give the next result.",
        "map" => "Runs its function on each item of a list: the list of results.",

        // ── numbers ──
        "math" => "One of JavaScript's Math functions of a number, chosen by fn: abs, sign, floor, ceil, trunc, round, sqrt, cbrt, exp, expm1, log, log2, log10, log1p, sin, cos, tan, their inverses and hyperbolic forms, fround, f16round, clz32. Produces nothing where JavaScript gives NaN or Infinity.",
        "max" => "The largest of its numbers (0 over -0). Takes more inputs with +.",
        "min" => "The smallest of its numbers (-0 under 0). Takes more inputs with +.",
        "aggregate" => "One number from a list of numbers, chosen by fn: its max, its min, hypot (the square root of the sum of the squares) or sum_precise (the exact sum, rounded once). The max or min of an empty list produces nothing.",
        "atan2" => "The angle from the x axis to the point (x, y), in radians: Math.atan2(y, x).",
        "power" => "a to the power b, as Math.pow(a, b). Produces nothing where JavaScript gives NaN or Infinity (0 to a negative power, a negative number to a fraction).",
        "remainder" => "What is left of a after dividing by b, with a's sign: a % b. Dividing by zero produces nothing.",
        "bitwise" => "A 32-bit integer operation on two numbers, chosen by op: and, or, xor, shl, shr, ushr (JavaScript's &, |, ^, <<, >>, >>>) or imul (Math.imul).",
        "number_test" => "Whether a value is a finite number, an integer, a safe integer or NaN, chosen by test. With coerce \"none\" only a number can pass (Number.isFinite); with \"number\" the value is read as a number first (isFinite, isNaN).",
        "to_number" => "A value read as a number, by grammar: number (Number(x)), float (parseFloat) or int (parseInt, in radix when it is wired). Produces nothing when it doesn't read as a finite number.",
        "number_format" => "A number written as text, by style: string (toString, in radix digits when wired), fixed (toFixed), precision (toPrecision) or exponential (toExponential), digits as each takes them. Digits out of range produce nothing.",

        // ── text ──
        "concat" => "Joins texts end to end. Takes more inputs with +.",
        "uppercase" => "The text in upper case.",
        "lowercase" => "The text in lower case.",
        "text_length" => "How many characters a text has.",
        "slice" => "Part of a text or a list, by mode. length: end characters or items from start (counting from 0). end: the items from start up to end, each counting from the end when negative, end the length when unwired (xs.slice(start, end)); a text in this mode produces nothing for now.",
        "index_of" => "Where needle first appears: in a list, the first item that equals it; in a text, counting characters from 0. −1 when it doesn't.",
        "split" => "Cuts a text at each sep into a list of pieces; join's inverse. An empty sep produces nothing.",

        // ── lists ──
        "list" => "A list of its inputs, in order. Takes more inputs with +.",
        "get" => "The item of a list at index (counting from 0); nothing past its end.",
        "append" => "A list with item added at the end.",
        "extend" => "A list with every item of items added at the end.",
        "length" => "How many items a list has.",
        "range" => "The numbers from 0 to n-1, as a list: map over it to do something n times. n is a whole number up to 10000; any other is nothing.",
        "sum" => "The total of a list of numbers.",
        "join" => "Joins a list of texts into one, with sep between each.",
        "at" => "The item of a list at index, counting from the end when it is negative (xs.at(-1) is the last); nothing out of range. A text produces nothing for now.",
        "indices" => "A list's positions: 0 to its length − 1 (xs.keys()).",
        "entries" => "A list's [index, item] pairs, or a record's [key, value] pairs in key order.",
        "any" => "Whether its function answers true for any item of a list, stopping at the first (xs.some). Its answer is read as JavaScript reads a condition: false, 0 and \"\" are false, anything else true.",
        "all" => "Whether its function answers true for every item of a list, stopping at the first false (xs.every).",
        "filter" => "The items of a list its function answers true for, in order.",
        "find" => "The first item of a list its function answers true for, stopping there; nothing when none does.",
        "find_index" => "The position of the first item its function answers true for, stopping there; −1 when none does.",
        "find_last" => "The last item of a list its function answers true for, looking from the end; nothing when none does.",
        "find_last_index" => "The position of the last item its function answers true for, looking from the end; −1 when none does.",
        "sort_by" => "The items of a list in the order its function puts them: given two items a and b, it answers a number, below 0 for a first, above 0 for b first, 0 to keep their order. Stable, and the same order every time. An answer that isn't a number produces nothing.",
        "includes" => "Whether an item of a list equals needle (lists and records by what they hold). A text produces nothing for now.",
        "last_index_of" => "The position of the last item of a list that equals needle; −1 when none does. A text produces nothing for now.",
        "reverse" => "The items of a list in reverse order.",
        "to_spliced" => "A list with count items from start replaced by the items of items: start counts from the end when negative; count unwired is the rest of the list, items unwired is none.",
        "with_item" => "A list with the item at index replaced by value, index counting from the end when negative; nothing out of range.",
        "fill" => "A list with the positions from start up to end set to value, each counting from the end when negative; start unwired is 0, end the length.",
        "copy_within" => "A list with the items from start up to end copied over those from target, as they were before; each counts from the end when negative, start unwired is 0, end the length.",
        "sort" => "The items of a list in JavaScript's default order: each written as text and compared character by character (so 10 comes before 9). Stable.",
        "flat" => "A list with the items of its lists spread into it, depth levels deep (a whole number, or all).",
        "is_list" => "Whether a value is a list.",
        "unique" => "Each item of a list once, where it first appears (lists and records by what they hold).",
        "pair" => "Two values held together as one.",
        "some" => "An optional value that is there: holds its input.",
        "none" => "An optional value that isn't there.",

        // ── records ──
        "record" => "An empty record, to add fields to with with_field.",
        "field" => "One field of a record, by the name set on the node; nothing when the record hasn't it.",
        "with_field" => "The record with a field set to value, by the name set on the node (added, or replaced).",
        "without_field" => "The record without the named field.",
        "keys" => "The names of a record's fields, in order.",
        "lookup" => "One field of a record, by a key that arrives on a wire (a route's path param, say); nothing when it isn't there.",
        "insert" => "The record with key set to value, the key arriving on a wire.",
        "remove" => "The record without key, the key arriving on a wire.",
        "values" => "A record's values, in key order: Object.values(r).",
        "from_entries" => "A record made of [key, value] pairs, a later key replacing an earlier: Object.fromEntries(es). Nothing when a pair isn't a list of a text key and a value.",
        "merge" => "Every field of its records, a later record's where two have one: { ...a, ...b }. Shallow: a record inside is replaced, not merged. Takes more records with +.",
        "has" => "Whether a record has the key, with any value, null included: k in r, Object.hasOwn(r, k).",
        "group_by" => "The items of a list in lists by the key its function answers for each, a text: Object.groupBy(xs, f). Each list keeps the items' order; the keys come sorted. A key that isn't a text produces nothing.",
        "same_value" => "Whether two values are the same as JavaScript's Object.is says: -0 isn't 0. Lists and records compare by what they hold.",
        "type_of" => "What JavaScript's typeof says of a value: \"number\", \"boolean\", \"string\" or \"object\" (a list, a record or null).",
        "json_parse" => "Reads JSON text into a value (records, lists, numbers, text…); nothing when it isn't valid JSON. With fn, a function of each key and value, applied from the inside out, its answer standing in for the value (JSON.parse(t, reviver)); one it answers nothing for is left out.",
        "json_stringify" => "Writes a value out as JSON text, as JavaScript's JSON.stringify: indent lays it out a member a line, keys (a JSON list) keeps only those keys of every record, in that order, and fn, a function of each key and value applied from the outside in, writes its answer instead (the replacer); one it answers nothing for is left out.",

        // ── signatures and encodings ──
        "sha256" => "The SHA-256 digest of a text, as hex or base64.",
        "hmac_sha256" => "The HMAC-SHA256 signature of message under key, as hex or base64: how webhooks are signed.",
        "hex" => "Encodes text as hex, or decodes hex back to text.",
        "base64" => "Encodes text as base64, or decodes it back; standard or URL-safe alphabet.",
        "secure_equals" => "Whether two texts are equal, compared in constant time: for checking signatures and tokens without leaking where they differ.",

        // ── any value ──
        "to_text" => "Any value written out as text, as a literal: a text comes out in quotes.",
        "as_text" => "Any value as it reads in a message: a text is itself, a number or true/false as written, a record or list as JSON. For putting a value into a text.",
        "is_truthy" => "Whether a value counts as true: not 0, false, empty text, an empty list or record, or none.",
        "guard" => "Passes value on only when cond is true; otherwise nothing, so what follows doesn't run.",
        "default" => "Passes its input on, or the value set on the node when the input didn't arrive: how a flow fills a gap.",
        "coalesce" => "Passes a on, or b when a didn't arrive: a fallback that is itself computed, where default's is a fixed value.",
        "equals" => "Whether two values are the same.",

        // ── outside the run ──
        "http_request" => "Calls the host set on the node (HTTPS) with the method, path, headers and body, and answers {status, headers, body}. Performed by the server between rounds, within the deployment's limits; the host is a permission it asks for. Holds back when a wired input didn't arrive.",
        "flow_call" => "Runs the action set on the node of the deployed flow set on the node, with the input (a value; {} when none is wired), and answers {answer, run} when it ran, or {error, message} when it didn't (not_found, not_running, no_action, waiting, failed, too_deep, refused). Performed by the server between rounds, in-process, as an ordinary action run of the other flow, logged there as run by this flow; the pair is a permission it asks for. Holds back when a wired input didn't arrive.",
        "flow_start" => "Starts the action set on the node of the deployed flow set on the node (this flow's own, or another's), with the input (a value; {} when none is wired), and doesn't wait for it to end: the server runs it in-process until it finishes or first waits, and answers then, {answer, run} when it finished, {run, waiting} when it waits (waiting names each point it waits at, with the token that resumes it), or {error, message} when it didn't run. Never waits itself, so a route hands out the link of a run that waits. The pair is a permission it asks for. Holds back when a wired input didn't arrive.",
        "http_stream" => "Calls the host set on the node like http_request, and the server forwards the answer's body to the route's caller as it arrives (a 2xx answer; the reply is 200 with the answer's content type): a model's tokens, say, as they come. With pipe set to a channel's name, the pieces go to that channel's subscribers instead, as they arrive (a shared live transcript). The flow still gets the whole answer, as http_request's.",
        "http_upload" => "Calls the host set on the node like http_request, with the file a handle names as the body: the server streams its bytes from the folder (one the deployment reads), never through the flow, and fails the call if the file changed since the run read it.",
        "file_arrival" => "The file in a folder that started the run: its handle {folder, name, size, hash, modified, event}, event new or changed; for a file deleted (on: deleted), as last seen, without a hash. A flow with one runs once for each file that arrives there and settles (or, on: deleted, is removed), its name matching the glob matching if set, not on requests.",
        "clock" => "The time the run started, in seconds since 1970. The same everywhere in one run, and recorded, so a replay sees it too.",
        "random" => "A random number from 0 up to 1, the same everywhere in one run, and recorded.",
        "secret" => "A secret's value, by the name set on the node, from the deployment's secrets (set under ⚙). Never logged: it shows as ‹secret:name›. Reading it is a permission.",
        "await" => "Waits for an event: the run parks here and continues, perhaps days later, when the event arrives (through its resume_token), or answers {expired: true} after expires seconds.",
        "resume_token" => "The token that delivers an event to the waiting point (await) named on the node: put it in a link or a callback. Never logged.",
        "store_read" => "The deployment's state under the key set on the node, as the run started. Reading it is a permission.",
        "store_write" => "Sets the deployment's state under the key set on the node to value, when the run finishes. Writing it is a permission.",
        "collection_get" => "The item of a collection under key, as the run found the collection; nothing when there is none. Reading it is a permission.",
        "collection_list" => "A record of a collection's items, by key, in key order, up to limit: those whose key starts with prefix, after the key after (both optional, for paging).",
        "collection_put" => "Puts value into a collection under key when the run finishes (added, or replaced). Writing it is a permission.",
        "collection_delete" => "Deletes a collection's item under key when the run finishes.",
        "file_read" => "The text of the file name in a folder, as the run found it; nothing when there is none. The folder is a name an admin binds to a real folder. Reading it is a permission.",
        "file_list" => "A record of a folder's files (name, size, hash, modified) in name order, up to limit: those whose name starts with prefix, after the name after (both optional, for paging).",
        "file_write" => "Writes text to the file name in a folder when the run finishes (created, or replaced whole). Writing it is a permission.",
        "file_delete" => "Deletes the file name in a folder when the run finishes.",
        "file_handle" => "A handle to the file name in a folder, of any size, without its bytes: {folder, name, size, hash, modified}; nothing when there is none. Copy it to another folder, or pass it on.",
        "file_lines" => "Up to limit lines of the file name in a folder, from line from (0 is the first; -100 is the 100th from the end), without their line endings. Only those lines are read into the flow; the log proves them part of the file.",
        "file_slice" => "length bytes of the file name in a folder from byte offset (from the end when negative), as text. Only those bytes are read into the flow; the log proves them part of the file.",
        "file_copy" => "Copies the file a handle names into this folder as name when the run finishes, host to host: its bytes never pass through the flow. Fails if the file changed since the run read it.",
        "file_create" => "Writes text to the file name in a folder when the run finishes, only if no file has that name then: it never replaces one (the run answers 500 if one is there).",
        "file_create_copy" => "Copies the file a handle names into this folder as name when the run finishes, only if no file has that name then: it never replaces one.",
        "db_query" => "The rows sql selects from a database, as records by column name, as the run found them; params are its $1, $2, …. The database is a name an admin binds. Reading each table it names is a permission.",
        "db_one" => "The first row sql selects from a database, as a record by column name; nothing when there is none. params are its $1, $2, ….",
        "db_find" => "The documents filter selects from a collection of a document database (MongoDB), as records, as the run found them; params are its $1, $2, …. sort orders them and limit caps how many. Reading the collection is a permission.",
        "db_find_one" => "The first document filter selects from a collection, as a record; nothing when there is none. params are its $1, $2, ….",
        "db_insert" => "Adds document to a collection when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, ….",
        "db_update" => "Applies update (its $set, $inc, … operators) to the documents filter selects, when the run finishes; params are their $1, $2, …. With expect, it must match exactly that many documents, or nothing the run did there lands.",
        "db_delete" => "Deletes the documents filter selects when the run finishes; params are its $1, $2, …. With expect, it must delete exactly that many, or nothing the run did there lands.",
        "db_exec" => "Runs sql (an insert, update or delete) on a database when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, …. With expect, it must change exactly that many rows, or nothing the run did there lands.",
        "channel_send" => "Sends pack on the channel set on the node once the run is over and its writes have landed: every caller subscribed to the channel gets it as a server-sent event. Sending on it is a permission.",
        "channel_subscribe" => "A subscription to the channel set on the node, for a route to answer with: the caller gets each pack sent on it from then on (from \"kept\": the packs the channel keeps first), as server-sent events. Letting callers subscribe is a permission.",
        "egress" => "Sends its payload out of the flow as a message, when the run finishes.",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_genesis_primitive_says_what_it_does() {
        for name in crate::GENESIS {
            let text = describe(name).unwrap_or_else(|| panic!("{name} has no description"));
            assert!(text.len() > 10 && text.ends_with('.'), "{name}: {text:?}");
        }
        assert_eq!(describe("no_such_leaf"), None);
    }
}
