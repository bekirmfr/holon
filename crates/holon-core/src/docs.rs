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

        // ── text ──
        "concat" => "Joins texts end to end. Takes more inputs with +.",
        "uppercase" => "The text in upper case.",
        "lowercase" => "The text in lower case.",
        "text_length" => "How many characters a text has.",
        "slice" => "Part of a text: len characters, from character start (counting from 0).",
        "index_of" => "Where needle first appears in haystack, counting characters from 0; −1 when it doesn't.",
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
        "find" => "The first record in a list whose field (named on the node) equals value; nothing when none does.",
        "json_parse" => "Reads JSON text into a value (records, lists, numbers, text…); nothing when it isn't valid JSON.",
        "json_stringify" => "Writes a value out as JSON text.",

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
        "http_upload" => "Calls the host set on the node like http_request, with the file a handle names as the body: the server streams its bytes from the folder (one the deployment reads), never through the flow, and fails the call if the file changed since the run read it.",
        "file_arrival" => "The handle of the new file in a folder that started the run: {folder, name, size, hash, modified}. A flow with one runs once for each file that arrives there (and settles), not on requests.",
        "clock" => "The time the run started, in seconds since 1970. The same everywhere in one run, and recorded, so a replay sees it too.",
        "random" => "A random number from 0 up to 1, the same everywhere in one run, and recorded.",
        "ingress" => "What arrived at the deployment (a request's body, a trigger's payload).",
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
