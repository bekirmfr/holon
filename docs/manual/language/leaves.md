# Leaves

<!-- Generated from the catalog by holon_author::agent::leaves_page: don't edit it by hand.
     HOLON_BLESS=1 cargo test -p holon-author --test manual writes it again. -->

Every building block a flow is made of. Each is shown as `name(inputs) -> outputs {params}`:
call a leaf by its name, with its inputs in order and its parameters by name (see
[Calls](reference.md#calls)). Many have a shorter written form (`a + b` is `add`, `x ?? y` is
`coalesce`, `"{x}"` is `concat`), and `holon fmt` writes that form; the
[reference](reference.md) has them all.

## Computing

These touch nothing outside: given the same inputs, they always give the same output.

[`const`](#const) · [`add`](#add) · [`multiply`](#multiply) · [`subtract`](#subtract) · [`divide`](#divide) · [`compare`](#compare) · [`and`](#and) · [`or`](#or) · [`not`](#not) · [`select`](#select) · [`cons`](#cons) · [`head`](#head) · [`tail`](#tail) · [`is_empty`](#is_empty) · [`fold`](#fold) · [`map`](#map) · [`concat`](#concat) · [`uppercase`](#uppercase) · [`lowercase`](#lowercase) · [`text_length`](#text_length) · [`slice`](#slice) · [`index_of`](#index_of) · [`split`](#split) · [`list`](#list) · [`get`](#get) · [`append`](#append) · [`extend`](#extend) · [`length`](#length) · [`sum`](#sum) · [`join`](#join) · [`range`](#range) · [`pair`](#pair) · [`some`](#some) · [`none`](#none) · [`record`](#record) · [`field`](#field) · [`with_field`](#with_field) · [`without_field`](#without_field) · [`keys`](#keys) · [`lookup`](#lookup) · [`insert`](#insert) · [`remove`](#remove) · [`find`](#find) · [`json_parse`](#json_parse) · [`json_stringify`](#json_stringify) · [`sha256`](#sha256) · [`hmac_sha256`](#hmac_sha256) · [`hex`](#hex) · [`base64`](#base64) · [`secure_equals`](#secure_equals) · [`to_text`](#to_text) · [`as_text`](#as_text) · [`is_truthy`](#is_truthy) · [`guard`](#guard) · [`default`](#default) · [`coalesce`](#coalesce) · [`equals`](#equals)

### `const`

```
const() -> out: Number  {value: 0, type: Number}
```

A fixed value set on the node: a number, true or false, text, or JSON. The type picks which.

### `add`

```
add(a: Number, b: Number) -> out: Number
```

Adds its numbers. Takes more inputs with +.

### `multiply`

```
multiply(a: Number, b: Number) -> out: Number
```

Multiplies its numbers. Takes more inputs with +.

### `subtract`

```
subtract(a: Number, b: Number) -> out: Number
```

Subtracts b from a: a − b.

### `divide`

```
divide(a: Number, b: Number) -> out: Number
```

Divides a by b: a ÷ b. Dividing by zero produces nothing.

### `compare`

```
compare(a: Number, b: Number) -> out: Boolean  {op: =}
```

Compares two numbers with the chosen operator (=, !=, <, <=, >, >=): true or false.

### `and`

```
and(a: Boolean, b: Boolean) -> out: Boolean
```

True when every input is true. Takes more inputs with +.

### `or`

```
or(a: Boolean, b: Boolean) -> out: Boolean
```

True when any input is true. Takes more inputs with +.

### `not`

```
not(v: Boolean) -> out: Boolean
```

The opposite of a true or false.

### `select`

```
select(cond: Boolean, if_true: Any, if_false: Any) -> out: Any
```

Passes on if_true when cond is true, if_false when it's false.

### `cons`

```
cons(item: Any, list: List<Any>) -> out: List<Any>
```

A list with item put first, before the items of list.

### `head`

```
head(list: List<Any>) -> out: Any
```

A list's first item; nothing for an empty list.

### `tail`

```
tail(list: List<Any>) -> out: List<Any>
```

A list without its first item; nothing for an empty list.

### `is_empty`

```
is_empty(list: List<Any>) -> out: Boolean
```

Whether a list has no items.

### `fold`

```
fold(init: Any, list: List<Any>) -> out: Any  {fn}
```

Runs its function over a list, carrying a result along: starting from init, each item and the result so far give the next result.

### `map`

```
map(list: List<Any>) -> out: List<Any>  {fn}
```

Runs its function on each item of a list: the list of results.

### `concat`

```
concat(a: Text, b: Text) -> out: Text
```

Joins texts end to end. Takes more inputs with +.

### `uppercase`

```
uppercase(s: Text) -> out: Text
```

The text in upper case.

### `lowercase`

```
lowercase(s: Text) -> out: Text
```

The text in lower case.

### `text_length`

```
text_length(s: Text) -> out: Number
```

How many characters a text has.

### `slice`

```
slice(s: Text, start: Number, len: Number) -> out: Text
```

Part of a text: len characters, from character start (counting from 0).

### `index_of`

```
index_of(haystack: Text, needle: Text) -> out: Number
```

Where needle first appears in haystack, counting characters from 0; −1 when it doesn't.

### `split`

```
split(text: Text, sep: Text) -> out: List<Text>
```

Cuts a text at each sep into a list of pieces; join's inverse. An empty sep produces nothing.

### `list`

```
list(a: Any, b: Any) -> out: List<Any>
```

A list of its inputs, in order. Takes more inputs with +.

### `get`

```
get(list: List<Any>, index: Number) -> out: Any
```

The item of a list at index (counting from 0); nothing past its end.

### `append`

```
append(list: List<Any>, item: Any) -> out: List<Any>
```

A list with item added at the end.

### `extend`

```
extend(list: List<Any>, items: List<Any>) -> out: List<Any>
```

A list with every item of items added at the end.

### `length`

```
length(list: List<Any>) -> out: Number
```

How many items a list has.

### `sum`

```
sum(list: List<Number>) -> out: Number
```

The total of a list of numbers.

### `join`

```
join(list: List<Text>, sep: Text) -> out: Text
```

Joins a list of texts into one, with sep between each.

### `range`

```
range(n: Number) -> out: List<Number>
```

The numbers from 0 to n-1, as a list: map over it to do something n times. n is a whole number up to 10000; any other is nothing.

### `pair`

```
pair(a: Any, b: Any) -> out: (Any, Any)
```

Two values held together as one.

### `some`

```
some(value: Any) -> out: Any | Unit
```

An optional value that is there: holds its input.

### `none`

```
none() -> out: Any | Unit
```

An optional value that isn't there.

### `record`

```
record() -> out: Record
```

An empty record, to add fields to with with_field.

### `field`

```
field(record: Record) -> out: Any  {name: id}
```

One field of a record, by the name set on the node; nothing when the record hasn't it.

### `with_field`

```
with_field(record: Record, value: Any) -> out: Record  {name: id}
```

The record with a field set to value, by the name set on the node (added, or replaced).

### `without_field`

```
without_field(record: Record) -> out: Record  {name: id}
```

The record without the named field.

### `keys`

```
keys(record: Record) -> out: List<Text>
```

The names of a record's fields, in order.

### `lookup`

```
lookup(record: Record, key: Text) -> out: Any
```

One field of a record, by a key that arrives on a wire (a route's path param, say); nothing when it isn't there.

### `insert`

```
insert(record: Record, key: Text, value: Any) -> out: Record
```

The record with key set to value, the key arriving on a wire.

### `remove`

```
remove(record: Record, key: Text) -> out: Record
```

The record without key, the key arriving on a wire.

### `find`

```
find(list: List<Any>, value: Any) -> out: Any  {name: id}
```

The first record in a list whose field (named on the node) equals value; nothing when none does.

### `json_parse`

```
json_parse(text: Text) -> out: Any
```

Reads JSON text into a value (records, lists, numbers, text…); nothing when it isn't valid JSON.

### `json_stringify`

```
json_stringify(value: Any) -> out: Text
```

Writes a value out as JSON text.

### `sha256`

```
sha256(text: Text) -> out: Text  {encoding: hex}
```

The SHA-256 digest of a text, as hex or base64.

### `hmac_sha256`

```
hmac_sha256(key: Text, message: Text) -> out: Text  {encoding: hex}
```

The HMAC-SHA256 signature of message under key, as hex or base64: how webhooks are signed.

### `hex`

```
hex(text: Text) -> out: Text  {mode: encode}
```

Encodes text as hex, or decodes hex back to text.

### `base64`

```
base64(text: Text) -> out: Text  {mode: encode, alphabet: standard}
```

Encodes text as base64, or decodes it back; standard or URL-safe alphabet.

### `secure_equals`

```
secure_equals(a: Text, b: Text) -> out: Boolean
```

Whether two texts are equal, compared in constant time: for checking signatures and tokens without leaking where they differ.

### `to_text`

```
to_text(value: Any) -> out: Text
```

Any value written out as text, as a literal: a text comes out in quotes.

### `as_text`

```
as_text(value: Any) -> out: Text
```

Any value as it reads in a message: a text is itself, a number or true/false as written, a record or list as JSON. For putting a value into a text.

### `is_truthy`

```
is_truthy(value: Any) -> out: Boolean
```

Whether a value counts as true: not 0, false, empty text, an empty list or record, or none.

### `guard`

```
guard(cond: Boolean, value: Any) -> out: Any
```

Passes value on only when cond is true; otherwise nothing, so what follows doesn't run.

### `default`

```
default(value: Any) -> out: Any  {value: 0, type: Number}
```

Passes its input on, or the value set on the node when the input didn't arrive: how a flow fills a gap.

### `coalesce`

```
coalesce(a: Any, b: Any) -> out: Any
```

Passes a on, or b when a didn't arrive: a fallback that is itself computed, where default's is a fixed value.

### `equals`

```
equals(a: Any, b: Any) -> out: Boolean
```

Whether two values are the same.

## The outside world

Each of these is something a flow may do, and so a line of what it asks for when it is deployed. The language writes most of them as nouns (`req`, `now`, `secret.x`, `state.x`, `http.get`, a collection or a folder).

[`http_request`](#http_request) · [`http_upload`](#http_upload) · [`http_stream`](#http_stream) · [`clock`](#clock) · [`random`](#random) · [`ingress`](#ingress) · [`secret`](#secret) · [`file_arrival`](#file_arrival) · [`await`](#await) · [`resume_token`](#resume_token) · [`store_read`](#store_read) · [`store_write`](#store_write) · [`collection_get`](#collection_get) · [`collection_list`](#collection_list) · [`collection_put`](#collection_put) · [`collection_delete`](#collection_delete) · [`file_read`](#file_read) · [`file_list`](#file_list) · [`file_write`](#file_write) · [`file_delete`](#file_delete) · [`file_handle`](#file_handle) · [`file_lines`](#file_lines) · [`file_slice`](#file_slice) · [`file_copy`](#file_copy) · [`file_create`](#file_create) · [`file_create_copy`](#file_create_copy) · [`db_query`](#db_query) · [`db_one`](#db_one) · [`db_exec`](#db_exec) · [`db_find`](#db_find) · [`db_find_one`](#db_find_one) · [`db_insert`](#db_insert) · [`db_update`](#db_update) · [`db_delete`](#db_delete) · [`channel_send`](#channel_send) · [`channel_subscribe`](#channel_subscribe) · [`egress`](#egress)

### `http_request`

```
http_request(path: Text, headers: Record, body: Any) -> out: Record  {method: GET, host: example.com}
```

Calls the host set on the node (HTTPS) with the method, path, headers and body, and answers {status, headers, body}. Performed by the server between rounds, within the deployment's limits; the host is a permission it asks for. Holds back when a wired input didn't arrive.

### `http_upload`

```
http_upload(path: Text, headers: Record, file: Record) -> out: Record  {method: PUT, host: example.com}
```

Calls the host set on the node like http_request, with the file a handle names as the body: the server streams its bytes from the folder (one the deployment reads), never through the flow, and fails the call if the file changed since the run read it.

### `http_stream`

```
http_stream(path: Text, headers: Record, body: Any) -> out: Record  {method: POST, host: example.com, pipe}
```

Calls the host set on the node like http_request, and the server forwards the answer's body to the route's caller as it arrives (a 2xx answer; the reply is 200 with the answer's content type): a model's tokens, say, as they come. With pipe set to a channel's name, the pieces go to that channel's subscribers instead, as they arrive (a shared live transcript). The flow still gets the whole answer, as http_request's.

### `clock`

```
clock() -> out: Number
```

The time the run started, in seconds since 1970. The same everywhere in one run, and recorded, so a replay sees it too.

### `random`

```
random() -> out: Number
```

A random number from 0 up to 1, the same everywhere in one run, and recorded.

### `ingress`

```
ingress() -> out: Any
```

What arrived at the deployment (a request's body, a trigger's payload).

### `secret`

```
secret() -> out: Text  {name: api_key}
```

A secret's value, by the name set on the node, from the deployment's secrets (set under ⚙). Never logged: it shows as ‹secret:name›. Reading it is a permission.

### `file_arrival`

```
file_arrival() -> out: Record  {folder: files, on: new, matching}
```

The file in a folder that started the run: its handle {folder, name, size, hash, modified, event}, event new or changed; for a file deleted (on: deleted), as last seen, without a hash. A flow with one runs once for each file that arrives there and settles (or, on: deleted, is removed), its name matching the glob matching if set, not on requests.

### `await`

```
await() -> out: Any  {name: event, expires: 604800}
```

Waits for an event: the run parks here and continues, perhaps days later, when the event arrives (through its resume_token), or answers {expired: true} after expires seconds.

### `resume_token`

```
resume_token() -> out: Text  {await: event}
```

The token that delivers an event to the waiting point (await) named on the node: put it in a link or a callback. Never logged.

### `store_read`

```
store_read() -> out: Any  {key: value}
```

The deployment's state under the key set on the node, as the run started. Reading it is a permission.

### `store_write`

```
store_write(value: Any) -> out: Effect  {key: value}
```

Sets the deployment's state under the key set on the node to value, when the run finishes. Writing it is a permission.

### `collection_get`

```
collection_get(key: Text) -> out: Any  {collection: items}
```

The item of a collection under key, as the run found the collection; nothing when there is none. Reading it is a permission.

### `collection_list`

```
collection_list(prefix: Text, after: Text) -> out: Record  {collection: items, limit: 100}
```

A record of a collection's items, by key, in key order, up to limit: those whose key starts with prefix, after the key after (both optional, for paging).

### `collection_put`

```
collection_put(key: Text, value: Any) -> out: Effect  {collection: items}
```

Puts value into a collection under key when the run finishes (added, or replaced). Writing it is a permission.

### `collection_delete`

```
collection_delete(key: Text) -> out: Effect  {collection: items}
```

Deletes a collection's item under key when the run finishes.

### `file_read`

```
file_read(name: Text) -> out: Text  {folder: files}
```

The text of the file name in a folder, as the run found it; nothing when there is none. The folder is a name an admin binds to a real folder. Reading it is a permission.

### `file_list`

```
file_list(prefix: Text, after: Text) -> out: Record  {folder: files, limit: 100}
```

A record of a folder's files (name, size, hash, modified) in name order, up to limit: those whose name starts with prefix, after the name after (both optional, for paging).

### `file_write`

```
file_write(name: Text, text: Text) -> out: Effect  {folder: files}
```

Writes text to the file name in a folder when the run finishes (created, or replaced whole). Writing it is a permission.

### `file_delete`

```
file_delete(name: Text) -> out: Effect  {folder: files}
```

Deletes the file name in a folder when the run finishes.

### `file_handle`

```
file_handle(name: Text) -> out: Record  {folder: files}
```

A handle to the file name in a folder, of any size, without its bytes: {folder, name, size, hash, modified}; nothing when there is none. Copy it to another folder, or pass it on.

### `file_lines`

```
file_lines(name: Text, from: Number) -> out: List<Text>  {folder: files, limit: 100}
```

Up to limit lines of the file name in a folder, from line from (0 is the first; -100 is the 100th from the end), without their line endings. Only those lines are read into the flow; the log proves them part of the file.

### `file_slice`

```
file_slice(name: Text, offset: Number, length: Number) -> out: Text  {folder: files}
```

length bytes of the file name in a folder from byte offset (from the end when negative), as text. Only those bytes are read into the flow; the log proves them part of the file.

### `file_copy`

```
file_copy(name: Text, from: Record) -> out: Effect  {folder: files}
```

Copies the file a handle names into this folder as name when the run finishes, host to host: its bytes never pass through the flow. Fails if the file changed since the run read it.

### `file_create`

```
file_create(name: Text, text: Text) -> out: Effect  {folder: files}
```

Writes text to the file name in a folder when the run finishes, only if no file has that name then: it never replaces one (the run answers 500 if one is there).

### `file_create_copy`

```
file_create_copy(name: Text, from: Record) -> out: Effect  {folder: files}
```

Copies the file a handle names into this folder as name when the run finishes, only if no file has that name then: it never replaces one.

### `db_query`

```
db_query(params: List<Any>) -> out: List<Record>  {database: db, sql: select * from t}
```

The rows sql selects from a database, as records by column name, as the run found them; params are its $1, $2, …. The database is a name an admin binds. Reading each table it names is a permission.

### `db_one`

```
db_one(params: List<Any>) -> out: Record  {database: db, sql: select * from t}
```

The first row sql selects from a database, as a record by column name; nothing when there is none. params are its $1, $2, ….

### `db_exec`

```
db_exec(params: List<Any>) -> out: Effect  {database: db, sql: delete from t where false, expect}
```

Runs sql (an insert, update or delete) on a database when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, …. With expect, it must change exactly that many rows, or nothing the run did there lands.

### `db_find`

```
db_find(params: List<Any>) -> out: List<Record>  {database: db, collection: items, filter: {}, sort, limit}
```

The documents filter selects from a collection of a document database (MongoDB), as records, as the run found them; params are its $1, $2, …. sort orders them and limit caps how many. Reading the collection is a permission.

### `db_find_one`

```
db_find_one(params: List<Any>) -> out: Record  {database: db, collection: items, filter: {}, sort}
```

The first document filter selects from a collection, as a record; nothing when there is none. params are its $1, $2, ….

### `db_insert`

```
db_insert(params: List<Any>) -> out: Effect  {database: db, collection: items, document: {}}
```

Adds document to a collection when the run finishes, in one transaction with the run's other writes there; params are its $1, $2, ….

### `db_update`

```
db_update(params: List<Any>) -> out: Effect  {database: db, collection: items, filter: {}, update: {"$set":{}}, expect}
```

Applies update (its $set, $inc, … operators) to the documents filter selects, when the run finishes; params are their $1, $2, …. With expect, it must match exactly that many documents, or nothing the run did there lands.

### `db_delete`

```
db_delete(params: List<Any>) -> out: Effect  {database: db, collection: items, filter: {}, expect}
```

Deletes the documents filter selects when the run finishes; params are its $1, $2, …. With expect, it must delete exactly that many, or nothing the run did there lands.

### `channel_send`

```
channel_send(pack: Any) -> out: Effect  {channel: updates}
```

Sends pack on the channel set on the node once the run is over and its writes have landed: every caller subscribed to the channel gets it as a server-sent event. Sending on it is a permission.

### `channel_subscribe`

```
channel_subscribe() -> out: Effect  {channel: updates, from: new}
```

A subscription to the channel set on the node, for a route to answer with: the caller gets each pack sent on it from then on (from "kept": the packs the channel keeps first), as server-sent events. Letting callers subscribe is a permission.

### `egress`

```
egress(payload: Any) -> out: Effect
```

Sends its payload out of the flow as a message, when the run finishes.

## Templates

Templates are written through the language's own forms (a route, `respond`, `stripe.verify`),
not called by name.

### `request`

A route: answers requests with the method and path set on the node (path params like :id). Gives the whole request, and its params, query, headers, raw body, and body parsed as JSON. With a respond it wires to, one route of the flow's server.

### `respond`

Answers the route's request: body as the response, with the status set on the node (a record or list is sent as JSON). When body didn't arrive, it answers the missing status instead (404 unless set) with an error.

### `stripe_verify`

Checks a Stripe webhook's signature as Stripe's own libraries do: the HMAC-SHA256 of the timestamp and raw body under the endpoint's secret, compared in constant time with each v1 signature (any one matching passes, as while a secret is rolled), the timestamp within tolerance seconds of now. Gives the event (the body parsed) only when it verifies; otherwise nothing, so a respond refuses it.
