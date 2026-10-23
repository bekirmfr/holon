// The `holon` command line: flows as text files (src/holonText.ts), checked and shown on any
// machine, a screen or not, through the same core and the same code as the editor. Installed
// as `holon` (from editor/: `npm run build:cli`, then `npm install -g .`); in the repository,
// `npm run holon -- <command>` runs it from source.
//
//   holon new <file> [--sample <name>]   write a starter flow (or a sample) to <file>
//   holon check <file>                   compile it: its routes, what it may do, or each mistake
//   holon show <file>                    an outline: routes, nodes and what feeds them
//   holon fmt <file>                     rewrite it in the canonical form
//
// A mistake is printed as `<file>:<line>: <message>`, and the exit code is 1.

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Edge } from "@xyflow/react";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import type { PrimitiveNodeType } from "../src/PrimitiveNode";
import { deriveServer, flowUnitHash } from "../src/canvasOps";
import { describeItem } from "../src/diagnostics";
import { compileHolon, HolonTextError, printHolon } from "../src/holonText";
import { buildSample, samples } from "../src/samples";

// The core: beside this file once built (dist-cli/), else the editor's own build (wasm/).
const here = dirname(fileURLToPath(import.meta.url));
const core = [join(here, "holon_wasm_bg.wasm"), join(here, "..", "wasm", "holon_wasm_bg.wasm")].find((p) => existsSync(p));
if (!core) {
  console.error("holon: the core (holon_wasm_bg.wasm) isn't beside it: build it with `npm run build:cli`");
  process.exit(1);
}
initSync({ module: readFileSync(core) });

const STARTER = `# A flow: one statement a line. Run \`holon check\` on it as you go.
flow my-flow
port 8200

node req   = request { method: GET, path: "/hello/:name" }
node name  = field { name: name }
node hi    = const { value: "Hello, ", type: Text }
node greet = concat
node res   = respond { status: "200", missing: "404" }

req.params  -> name.record
hi.out      -> greet.a
name.out    -> greet.b
req.request -> res.request
greet.out   -> res.body
`;

function usage(): never {
  console.error(`usage: holon <command> <file>

  new <file> [--sample <name>]   write a starter flow (or a sample: ${samples().map((s) => s.name).join(", ")})
  check <file>                   compile it: its routes, what it may do, or each mistake
  show <file>                    an outline: routes, nodes and what feeds them
  fmt <file>                     rewrite it in the canonical form`);
  process.exit(2);
}

const [command, file, ...rest] = process.argv.slice(2);
if (!command || !file) usage();

/** The file compiled, or its mistake printed (and exit 1). */
function compile(path: string) {
  if (!existsSync(path)) {
    console.error(`${path}: no such file`);
    process.exit(1);
  }
  const ev = new Evaluator();
  try {
    return { ev, ...compileHolon(readFileSync(path, "utf8"), ev) };
  } catch (e) {
    if (e instanceof HolonTextError) console.error(`${path}:${e.line}: ${e.message.replace(/^line \d+: /, "")}`);
    else console.error(`${path}: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
  }
}

const nodesOf = (doc: { nodes: unknown[] }) => doc.nodes as PrimitiveNodeType[];
const edgesOf = (doc: { edges: unknown[] }) => doc.edges as Edge[];

/** What the flow derives to: its server's routes (or none), its unit, what it may do. */
function derive(ev: Evaluator, doc: ReturnType<typeof compile>["doc"]) {
  const port = doc.server?.port ?? "";
  const server = deriveServer(nodesOf(doc), edgesOf(doc), ev, port);
  let hash = "";
  let caps: string[] = [];
  let error = "";
  try {
    hash = flowUnitHash(nodesOf(doc), edgesOf(doc), ev, port);
    caps = (ev.capability_names(hash) as string[]).sort();
  } catch (e) {
    error = e instanceof Error ? e.message : String(e);
  }
  return { server, hash, caps, error };
}

switch (command) {
  case "new": {
    if (existsSync(file)) {
      console.error(`${file} exists: not overwritten`);
      process.exit(1);
    }
    const sampleAt = rest.indexOf("--sample");
    let text = STARTER;
    if (sampleAt >= 0) {
      const sample = samples().find((s) => s.name === rest[sampleAt + 1]);
      if (!sample) usage();
      const ev = new Evaluator();
      text = `# ${sample.title}: ${sample.description}\n${printHolon(sample.name, buildSample(sample, ev), ev)}`;
    }
    writeFileSync(file, text);
    console.log(`wrote ${file}`);
    break;
  }
  case "check": {
    const { ev, name, doc } = compile(file);
    const { server, hash, caps, error } = derive(ev, doc);
    const problems = [...server.errors, ...(error ? [error] : [])];
    console.log(`${name ?? file}: ${nodesOf(doc).length} nodes, ${edgesOf(doc).length} wires`);
    if (server.endpoints.length > 0) {
      console.log(`a server on :${doc.server?.port || "?"}`);
      for (const e of server.endpoints) console.log(`  ${e.method} ${e.path}`);
    }
    if (hash) console.log(`unit ${hash}`);
    console.log(caps.length ? "it may:" : "it only computes: it reads and changes nothing outside");
    for (const c of caps) console.log(`  ${describeItem(c)}`);
    if (problems.length) {
      for (const p of problems) console.error(`${file}: ${p}`);
      process.exit(1);
    }
    break;
  }
  case "show": {
    // Each node by its name in the text, as the compiler says.
    const { ev, name, doc, names: nameOfNode } = compile(file);
    const { server } = derive(ev, doc);
    console.log(`${name ?? file}${doc.server?.port ? `  (a server on :${doc.server.port})` : ""}`);
    if (server.endpoints.length) {
      console.log("  routes");
      for (const e of server.endpoints) console.log(`    ${e.method} ${e.path}`);
    }
    console.log("  nodes");
    const nodes = nodesOf(doc);
    const width = Math.max(...nodes.map((n) => (nameOfNode.get(n.id) ?? n.id).length), 4);
    for (const n of nodes) {
      const params = n.data.params.filter((p) => p.kind === "Frozen" && p.value !== "").map((p) => `${p.name}=${p.kind === "Frozen" ? JSON.stringify(p.value) : ""}`).join(" ");
      const feeds = edgesOf(doc)
        .filter((e) => e.target === n.id)
        .map((e) => `${e.targetHandle} ← ${nameOfNode.get(e.source) ?? e.source}.${e.sourceHandle}`);
      console.log(`    ${(nameOfNode.get(n.id) ?? n.id).padEnd(width)}  ${n.data.name}${params ? `  ${params}` : ""}`);
      for (const f of feeds) console.log(`    ${"".padEnd(width)}    ${f}`);
    }
    break;
  }
  case "fmt": {
    const { ev, name, doc } = compile(file);
    writeFileSync(file, printHolon(name, doc, ev));
    console.log(`formatted ${file}`);
    break;
  }
  default:
    usage();
}
