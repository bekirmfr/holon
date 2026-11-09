// The palette's groups of genesis primitives (catalog.rs, §13a): names from GENESIS, in groups
// that follow its comment sections, some small adjacent ones merged for a manageable count. Every
// primitive is in exactly one (samples.test.ts checks it against the catalog).

export const CATALOG_GROUPS: readonly { name: string; items: ReadonlySet<string> }[] = [
  {
    name: "net",
    items: new Set(["http_request", "http_upload", "http_stream", "flow_call", "flow_start"]),
  },
  {
    name: "core",
    items: new Set([
      "const", "add", "multiply", "subtract", "divide",
      "compare", "and", "or", "not", "select",
      "cons", "head", "tail", "is_empty", "fold", "map",
    ]),
  },
  {
    name: "number",
    items: new Set([
      "math", "max", "min", "aggregate", "atan2", "power", "remainder",
      "bitwise", "number_test", "to_number", "number_format",
    ]),
  },
  {
    name: "date",
    items: new Set([
      "date_part", "date_utc", "date_set", "date_text", "date_parse",
      "date_add", "business_day_add", "business_days_between", "is_business_day",
    ]),
  },
  {
    name: "text",
    items: new Set([
      "concat", "uppercase", "lowercase", "text_length", "slice", "index_of", "split",
      "text_at", "code_at", "from_codes", "text_contains", "trim", "pad", "repeat", "replace", "normalize", "uri_encode", "uri_decode", "is_well_formed",
    ]),
  },
  {
    name: "list",
    items: new Set([
      "list", "get", "append", "extend", "length", "sum", "join", "range", "pair", "some", "none",
      "at", "indices", "entries", "any", "all", "filter", "find", "find_index", "find_last", "find_last_index",
      "sort_by", "includes", "last_index_of", "reverse", "to_spliced", "with_item", "fill", "copy_within",
      "sort", "flat", "is_list", "unique",
    ]),
  },
  {
    name: "record",
    items: new Set([
      "record", "field", "with_field", "without_field", "keys", "lookup", "insert", "remove",
      "values", "from_entries", "merge", "has", "group_by", "same_value", "type_of",
    ]),
  },
  {
    name: "encode",
    items: new Set([
      "json_parse", "json_stringify", "csv_parse", "csv_format",
      "xml_parse", "xml_format", "xml_find", "xml_text", "xml_attribute",
      "sha256", "hmac_sha256", "hex", "base64", "secure_equals", "jwt_verify", "jwt_decode",
      "pgp_encrypt", "pgp_verify", "pgp_key",
    ]),
  },
  {
    name: "bytes",
    items: new Set(["text_to_bytes", "bytes_to_text", "bytes_length", "bytes_concat", "bytes_to_list", "list_to_bytes", "gzip", "gunzip", "zip", "unzip"]),
  },
  {
    name: "decimal",
    items: new Set(["decimal_add", "decimal_sub", "decimal_mul", "decimal_div", "decimal_round", "decimal_sum", "decimal_compare"]),
  },
  {
    name: "util",
    items: new Set(["to_text", "as_text", "is_truthy", "guard", "default", "coalesce", "equals", "fail"]),
  },
  {
    name: "drivers",
    items: new Set(["clock", "random", "random_bytes", "secret", "file_arrival", "mail_arrival", "feed_event", "queue_message", "await", "resume_token"]),
  },
  {
    name: "state",
    items: new Set(["store_read", "store_write",
      "collection_get", "collection_list", "collection_put", "collection_delete"]),
  },
  {
    name: "files",
    items: new Set([
      "file_read", "file_list", "file_write", "file_delete",
      "file_handle", "file_lines", "file_slice", "file_copy",
      "file_create", "file_create_copy", "file_read_bytes",
    ]),
  },
  {
    name: "database",
    items: new Set([
      "db_query", "db_one", "db_exec",
      "db_find", "db_find_one", "db_insert", "db_update", "db_delete",
    ]),
  },
  {
    name: "channel",
    items: new Set(["channel_send", "channel_subscribe", "egress"]),
  },
  {
    name: "mail",
    items: new Set(["mail_send"]),
  },
  {
    name: "queue",
    items: new Set(["queue_send"]),
  },
];
