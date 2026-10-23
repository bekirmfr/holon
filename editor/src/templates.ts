// HTTP node templates: `request` and `respond`. They are not new primitives. Each is a small
// composite of existing genesis leaves (field, json_parse, record, with_field, const, default), rebuilt
// from the node's params whenever it is hashed, the same way a primitive node is rebuilt from its
// catalog entry. Pure (types only from ./holon), so it is testable without the wasm package.
//
// - `request` (params: method, path) takes the endpoint's request record on one input, hidden on
//   the canvas: it is fed by the endpoint the route derives into (derive.ts), never by a wire.
//   Its outputs are the request's parts: the whole record, path params, query, headers, the raw
//   body text, and the body parsed as JSON.
// - `respond` (params: status, missing) builds the `{status, body}` envelope the data plane sends.
//   It also takes the request (from the request node), which it doesn't read: that wire is what
//   makes a route one connected group even when the handler never looks at the request, e.g. a
//   listing. Status 204 has no body input. When the body input is absent (a lookup that found
//   nothing), it answers its `missing` status instead (404 by default) with `{"error": …}`. A
//   respond node without `missing` (placed before it existed) answers nothing, sent as a 500.
// - `stripe_verify` (param: tolerance, seconds) checks a Stripe webhook's signature, as Stripe's
//   own libraries do: the `stripe-signature` header (`t=<unix time>,v1=<hex HMAC>,…`) taken apart
//   into a record (split on "," then each piece on "=", folded into a record), the HMAC-SHA256
//   of `<t>.<raw body>` under the endpoint's signing secret compared with `v1` in constant time,
//   and `t` no older than `tolerance` (300 s, Stripe's default) as of `now`. Inputs: the request,
//   the secret (a `secret` node), and `now` (a `clock` node: the template stays pure, so a replay
//   checks against the time the run saw). Outputs: `event`, the body parsed as JSON, only when
//   verified (absent otherwise, so a `respond` with `missing` 400 refuses it); and `verified`
//   (absent when the header is missing or malformed). With several `v1` (Stripe signs with both
//   while a secret is being rolled), any one matching passes: each piece is compared with
//   `v1=<expected>` in constant time, folded into whether any matched.

import type { WEdge, WParam, WPort, WUnit } from "./holon";

const BOUNDARY = 4294967295;

export const TEMPLATES = ["request", "respond", "stripe_verify"] as const;
export type TemplateName = (typeof TEMPLATES)[number];

export function isTemplate(name: string): name is TemplateName {
  return (TEMPLATES as readonly string[]).includes(name);
}

/** What each template does, for the person placing it (core describes the primitives). */
export const TEMPLATE_DOCS: Record<TemplateName, string> = {
  request:
    "A route: answers requests with the method and path set on the node (path params like :id). Gives the whole request, and its params, query, headers, raw body, and body parsed as JSON. With a respond it wires to, one route of the flow's server.",
  respond:
    "Answers the route's request: body as the response, with the status set on the node (a record or list is sent as JSON). When body didn't arrive, it answers the missing status instead (404 unless set) with an error.",
  stripe_verify:
    "Checks a Stripe webhook's signature as Stripe's own libraries do: the HMAC-SHA256 of the timestamp and raw body under the endpoint's secret, compared in constant time with each v1 signature (any one matching passes, as while a secret is rolled), the timestamp within tolerance seconds of now. Gives the event (the body parsed) only when it verifies; otherwise nothing, so a respond refuses it.",
};

const frozen = (name: string, value: string): WParam => ({ kind: "Frozen", name, value });
const param = (params: WParam[], name: string) => {
  const p = params.find((x) => x.name === name);
  return p && p.kind === "Frozen" ? p.value : undefined;
};

export const REQUEST_OUTPUTS: WPort[] = [
  { name: "request", ty: "Record" },
  { name: "params", ty: "Record" },
  { name: "query", ty: "Record" },
  { name: "headers", ty: "Record" },
  { name: "body", ty: "Text" },
  { name: "json", ty: "Any" },
];

/** A template node's interface for a given set of params (a respond node's inputs depend on its
 *  status). `hiddenInputs`: the inputs that are fed by the endpoint, not drawn as handles. */
export function templateInterface(
  name: TemplateName,
  params: WParam[],
): { inputs: WPort[]; outputs: WPort[]; hiddenInputs: boolean } {
  if (name === "request") {
    return { inputs: [{ name: "request", ty: "Record" }], outputs: REQUEST_OUTPUTS, hiddenInputs: true };
  }
  if (name === "stripe_verify") {
    return {
      inputs: [
        { name: "request", ty: "Record" },
        { name: "secret", ty: "Text" },
        { name: "now", ty: "Number" },
      ],
      outputs: [
        { name: "event", ty: "Any" },
        { name: "verified", ty: "Boolean" },
      ],
      hiddenInputs: false,
    };
  }
  const noBody = param(params, "status") === "204";
  const request: WPort = { name: "request", ty: "Record" };
  return {
    inputs: noBody ? [request] : [request, { name: "body", ty: "Any" }],
    outputs: [{ name: "response", ty: "Record" }],
    hiddenInputs: false,
  };
}

/** The params a freshly dropped template node starts with. */
export function templateDefaults(name: TemplateName): WParam[] {
  if (name === "stripe_verify") return [frozen("tolerance", "300")];
  return name === "request"
    ? [frozen("method", "GET"), frozen("path", "/"), frozen("resume", "")]
    : [frozen("status", "200"), frozen("missing", "404")];
}

/**
 * The composite a template node stands for. `catalog` supplies each leaf's interface (so a leaf
 * here hashes exactly like the same leaf dropped from the palette); `insert` stores a child and
 * returns its hash. Returns an error message for params the template can't build from.
 */
export function buildTemplate(
  name: TemplateName,
  params: WParam[],
  catalog: Map<string, WUnit>,
  insert: (u: WUnit) => string,
): WUnit | string {
  const leaf = (prim: string, p: WParam[] = [], ins?: WPort[]): string => {
    const u = catalog.get(prim);
    if (!u) throw new Error(`"${prim}" is not in the catalog`);
    return insert({ ...u, params: p, ...(ins ? { inputs: ins } : {}) });
  };
  const e = (fc: number, fp: number, tc: number, tp: number): WEdge => ({
    from_child: fc,
    from_port: fp,
    to_child: tc,
    to_port: tp,
  });
  const { inputs, outputs } = templateInterface(name, params);

  if (name === "request") {
    const children = [
      leaf("field", [frozen("name", "params")]),
      leaf("field", [frozen("name", "query")]),
      leaf("field", [frozen("name", "headers")]),
      leaf("field", [frozen("name", "body")]),
      leaf("json_parse"),
    ];
    const edges = [
      e(BOUNDARY, 0, 0, 0),
      e(BOUNDARY, 0, 1, 0),
      e(BOUNDARY, 0, 2, 0),
      e(BOUNDARY, 0, 3, 0),
      e(3, 0, 4, 0),
      e(BOUNDARY, 0, BOUNDARY, 0), // request: the record itself
      e(0, 0, BOUNDARY, 1),
      e(1, 0, BOUNDARY, 2),
      e(2, 0, BOUNDARY, 3),
      e(3, 0, BOUNDARY, 4),
      e(4, 0, BOUNDARY, 5),
    ];
    return { body: { kind: "Composite", children, edges }, params: [], inputs, outputs };
  }

  if (name === "stripe_verify") {
    const tolerance = param(params, "tolerance") ?? "";
    if (!/^\d{1,6}$/.test(tolerance) || Number(tolerance) < 1) {
      return `tolerance "${tolerance}" is not a number of seconds (1–999999)`;
    }
    const text = (v: string) => leaf("const", [frozen("value", v), frozen("type", "Text")]);
    const number = (v: string) => leaf("const", [frozen("value", v), frozen("type", "Number")]);
    // (record, "k=v") → record with k set to v: what the header's pieces fold into.
    const pair = insert({
      body: {
        kind: "Composite",
        children: [leaf("split"), text("="), leaf("get"), leaf("get"), number("0"), number("1"), leaf("insert")],
        edges: [
          e(BOUNDARY, 1, 0, 0),
          e(1, 0, 0, 1),
          e(0, 0, 2, 0),
          e(4, 0, 2, 1),
          e(0, 0, 3, 0),
          e(5, 0, 3, 1),
          e(BOUNDARY, 0, 6, 0),
          e(2, 0, 6, 1),
          e(3, 0, 6, 2),
          e(6, 0, BOUNDARY, 0),
        ],
      },
      params: [],
      inputs: [
        { name: "acc", ty: "Record" },
        { name: "item", ty: "Text" },
      ],
      outputs: [{ name: "acc", ty: "Record" }],
    });
    // ({want, ok}, piece) → ok once any piece equals want (`v1=<expected>`), each compared in
    // constant time: what the header's pieces fold into for the signature check.
    const anyMatch = insert({
      body: {
        kind: "Composite",
        children: [
          leaf("field", [frozen("name", "want")]), // 0
          leaf("secure_equals"), // 1
          leaf("field", [frozen("name", "ok")]), // 2
          leaf("or"), // 3
          leaf("with_field", [frozen("name", "ok")]), // 4
        ],
        edges: [
          e(BOUNDARY, 0, 0, 0),
          e(BOUNDARY, 1, 1, 0),
          e(0, 0, 1, 1),
          e(BOUNDARY, 0, 2, 0),
          e(2, 0, 3, 0),
          e(1, 0, 3, 1),
          e(BOUNDARY, 0, 4, 0),
          e(3, 0, 4, 1),
          e(4, 0, BOUNDARY, 0),
        ],
      },
      params: [],
      inputs: [
        { name: "acc", ty: "Record" },
        { name: "item", ty: "Text" },
      ],
      outputs: [{ name: "acc", ty: "Record" }],
    });
    const three: WPort[] = [
      { name: "a", ty: "Text" },
      { name: "b", ty: "Text" },
      { name: "c", ty: "Text" },
    ];
    const children = [
      leaf("field", [frozen("name", "headers")]), // 0
      leaf("field", [frozen("name", "stripe-signature")]), // 1
      text(","), // 2
      leaf("split"), // 3: the header's pieces
      leaf("record"), // 4
      leaf("fold", [frozen("fn", pair)]), // 5: … as a record
      leaf("field", [frozen("name", "t")]), // 6
      text("v1="), // 7
      leaf("field", [frozen("name", "body")]), // 8: the raw body, exactly as sent
      text("."), // 9
      leaf("concat", [], three), // 10: t.body, what Stripe signed
      leaf("hmac_sha256", [frozen("encoding", "hex")]), // 11
      leaf("field", [frozen("name", "ok")]), // 12: a v1 signature matches
      leaf("json_parse"), // 13: t as a number
      leaf("subtract"), // 14: its age
      number(tolerance), // 15
      leaf("compare", [frozen("op", "<=")]), // 16: recent enough
      leaf("and"), // 17: verified
      leaf("json_parse"), // 18: the event
      leaf("guard"), // 19: … only when verified
      leaf("concat"), // 20: v1=<expected>
      leaf("record"), // 21
      leaf("with_field", [frozen("name", "want")]), // 22
      leaf("const", [frozen("value", "false"), frozen("type", "Boolean")]), // 23
      leaf("with_field", [frozen("name", "ok")]), // 24: {want, ok: false}
      leaf("fold", [frozen("fn", anyMatch)]), // 25: … over every piece of the header
    ];
    const edges = [
      e(BOUNDARY, 0, 0, 0),
      e(0, 0, 1, 0),
      e(1, 0, 3, 0),
      e(2, 0, 3, 1),
      e(4, 0, 5, 0),
      e(3, 0, 5, 1),
      e(5, 0, 6, 0),
      e(5, 0, 7, 0),
      e(BOUNDARY, 0, 8, 0),
      e(6, 0, 10, 0),
      e(9, 0, 10, 1),
      e(8, 0, 10, 2),
      e(BOUNDARY, 1, 11, 0),
      e(10, 0, 11, 1),
      e(7, 0, 20, 0),
      e(11, 0, 20, 1),
      e(21, 0, 22, 0),
      e(20, 0, 22, 1),
      e(22, 0, 24, 0),
      e(23, 0, 24, 1),
      e(24, 0, 25, 0),
      e(3, 0, 25, 1),
      e(25, 0, 12, 0),
      e(6, 0, 13, 0),
      e(BOUNDARY, 2, 14, 0),
      e(13, 0, 14, 1),
      e(14, 0, 16, 0),
      e(15, 0, 16, 1),
      e(12, 0, 17, 0),
      e(16, 0, 17, 1),
      e(8, 0, 18, 0),
      e(17, 0, 19, 0),
      e(18, 0, 19, 1),
      e(19, 0, BOUNDARY, 0),
      e(17, 0, BOUNDARY, 1),
    ];
    return { body: { kind: "Composite", children, edges }, params: [], inputs, outputs };
  }

  const status = param(params, "status") ?? "";
  const n = Number(status);
  if (!/^\d{3}$/.test(status) || n < 100 || n > 599) return `status "${status}" is not an HTTP status (100–599)`;
  const missing = param(params, "missing") ?? "";
  const m = Number(missing);
  if (missing !== "" && (!/^\d{3}$/.test(missing) || m < 400 || m > 599)) {
    return `missing "${missing}" is not an error status (400–599)`;
  }
  const children = [
    leaf("record"),
    leaf("const", [frozen("value", status), frozen("type", "Number")]),
    leaf("with_field", [frozen("name", "status")]),
  ];
  const edges = [e(0, 0, 2, 0), e(1, 0, 2, 1)];
  // Input 0, the request, is deliberately left unread (see the file header).
  if (inputs.length === 1) {
    edges.push(e(2, 0, BOUNDARY, 0));
  } else {
    children.push(leaf("with_field", [frozen("name", "body")]));
    edges.push(e(2, 0, 3, 0), e(BOUNDARY, 1, 3, 1));
    if (missing === "") {
      edges.push(e(3, 0, BOUNDARY, 0));
    } else {
      // No body, no envelope: `default` stands in the error envelope (it fires on an absent input).
      const error = m === 404 ? "not found" : "no value to answer with";
      const fallback = JSON.stringify({ status: m, body: { error } });
      children.push(leaf("default", [frozen("value", fallback), frozen("type", "Json")]));
      edges.push(e(3, 0, 4, 0), e(4, 0, BOUNDARY, 0));
    }
  }
  return { body: { kind: "Composite", children, edges }, params: [], inputs, outputs };
}
