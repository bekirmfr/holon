// HTTP node templates: `request` and `respond`. They are not new primitives. Each is a small
// composite of existing genesis leaves (field, json_parse, record, with_field, const), rebuilt
// from the node's params whenever it is hashed, the same way a primitive node is rebuilt from its
// catalog entry. Pure (types only from ./holon), so it is testable without the wasm package.
//
// - `request` (params: method, path) takes the endpoint's request record on one input, hidden on
//   the canvas: it is fed by the endpoint the route derives into (derive.ts), never by a wire.
//   Its outputs are the request's parts: the whole record, path params, query, headers, the raw
//   body text, and the body parsed as JSON.
// - `respond` (param: status) builds the `{status, body}` envelope the data plane sends. It also
//   takes the request (from the request node), which it doesn't read: that wire is what makes a
//   route one connected group even when the handler never looks at the request, e.g. a listing.
//   Status 204 has no body input.

import type { WEdge, WParam, WPort, WUnit } from "./holon";

const BOUNDARY = 4294967295;

export const TEMPLATES = ["request", "respond"] as const;
export type TemplateName = (typeof TEMPLATES)[number];

export function isTemplate(name: string): name is TemplateName {
  return (TEMPLATES as readonly string[]).includes(name);
}

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
  return name === "request" ? [frozen("method", "GET"), frozen("path", "/")] : [frozen("status", "200")];
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
  const leaf = (prim: string, p: WParam[] = []): string => {
    const u = catalog.get(prim);
    if (!u) throw new Error(`"${prim}" is not in the catalog`);
    return insert({ ...u, params: p });
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

  const status = param(params, "status") ?? "";
  const n = Number(status);
  if (!/^\d{3}$/.test(status) || n < 100 || n > 599) return `status "${status}" is not an HTTP status (100–599)`;
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
    edges.push(e(2, 0, 3, 0), e(BOUNDARY, 1, 3, 1), e(3, 0, BOUNDARY, 0));
  }
  return { body: { kind: "Composite", children, edges }, params: [], inputs, outputs };
}
