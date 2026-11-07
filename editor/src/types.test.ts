// @vitest-environment node
// holon.d.ts and the tsconfig.json beside it, as `holon types` writes them (the samples' folder
// keeps both), against TypeScript's own checker: every sample and every flow of the manual
// type-checks, and the mistakes in a flow TypeScript should catch, it catches (typescript-syntax
// L4 §13). Where the two checkers disagree, Holon's compiler decides and holon.d.ts is fixed.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { PAGES, flowsOf } from "./manual";

const SAMPLES = join(process.cwd(), "..", "crates", "holon-author", "samples");

/**
 * What TypeScript refuses and Holon means: `{ total } ?? status(400)` and `a / b ?? status(400)`
 * are reachable in Holon, where a record with a field that has no value has none, and so does a
 * division by zero (reference.md, How a flow runs). TypeScript 5.6 calls the right operand
 * unreachable for any left operand it reads as never nullish, and no setting turns that off.
 */
const DIFFERENCES = new Set([2869]);

/** TypeScript's diagnostics for these flows, by file, with holon.d.ts and the samples' tsconfig. */
function check(flows: Record<string, string>): Map<string, ts.Diagnostic[]> {
  const config = ts.parseConfigFileTextToJson("tsconfig.json", readFileSync(join(SAMPLES, "tsconfig.json"), "utf8"));
  const { options, errors } = ts.convertCompilerOptionsFromJson(config.config.compilerOptions, SAMPLES);
  expect(errors).toEqual([]);
  const files = new Map(Object.entries({ "/flows/holon.d.ts": readFileSync(join(SAMPLES, "holon.d.ts"), "utf8"), ...flows }));
  const host = ts.createCompilerHost(options);
  const fromDisk = host.getSourceFile.bind(host);
  host.getSourceFile = (name, language, ...rest) =>
    files.has(name) ? ts.createSourceFile(name, files.get(name)!, language) : fromDisk(name, language, ...rest);
  host.fileExists = (name) => files.has(name) || ts.sys.fileExists(name);
  host.readFile = (name) => files.get(name) ?? ts.sys.readFile(name);
  const program = ts.createProgram([...files.keys()], options, host);
  const out = new Map<string, ts.Diagnostic[]>();
  for (const d of ts.getPreEmitDiagnostics(program)) {
    const file = d.file?.fileName ?? "(options)";
    out.set(file, [...(out.get(file) ?? []), d]);
  }
  return out;
}

function said(d: ts.Diagnostic): string {
  const at = d.file && d.start !== undefined ? d.file.getLineAndCharacterOfPosition(d.start) : undefined;
  return `${d.file?.fileName ?? ""}${at ? `:${at.line + 1}:${at.character + 1}` : ""}: TS${d.code} ${ts.flattenDiagnosticMessageText(d.messageText, " ")}`;
}

/** Every diagnostic but Holon's differences, said. */
function refused(found: Map<string, ts.Diagnostic[]>): string[] {
  return [...found.values()].flat().filter((d) => !DIFFERENCES.has(d.code)).map(said);
}

describe("holon.d.ts, read by TypeScript", () => {
  it("type-checks every sample", () => {
    const samples = readdirSync(SAMPLES).filter((f) => f.endsWith(".holon.ts"));
    expect(samples.length).toBeGreaterThan(50);
    const flows = Object.fromEntries(samples.map((f) => [`/flows/${f}`, readFileSync(join(SAMPLES, f), "utf8")]));
    expect(refused(check(flows))).toEqual([]);
  });

  it("type-checks every flow of the manual", () => {
    const flows: Record<string, string> = {};
    for (const page of PAGES.keys()) {
      flowsOf(page).forEach((text, i) => (flows[`/manual/${page.replace(/[/.]/g, "_")}_${i + 1}.holon.ts`] = text));
    }
    expect(Object.keys(flows).length).toBeGreaterThan(30);
    expect(refused(check(flows))).toEqual([]);
  });

  // Each mistake: a flow that is right but for its first line.
  const head = `import { every, flow, route, status } from "holon"\nimport api from "host:api.example.com"\nflow("mistake", { port: 8300 })\n`;
  const mistakes: Record<string, string> = {
    "a method a route hasn't": `route.pots("/x", () => "ok")`,
    "a path without its /": `route.get("x", () => "ok")`,
    "a duration without its unit": `every("10", () => {})`,
    "a log level that isn't one": `route.get("/x", () => "ok", { log: "lazzy" })`,
    "a setting a flow hasn't": `flow("again", { prot: 8301 })`,
    "a limit a flow can't set": `flow("again", { limits: { run_budget: 5 } })`,
    "a method a host hasn't": `route.get("/x", async () => (await api.fetch("/y")).body)`,
    "a request read before it answers": `route.get("/x", () => api.get("/y").body)`,
    "a global a flow hasn't": `route.get("/x", () => fetch("/y"))`,
    "an import source Holon hasn't": `import key from "secrets:key"`,
    "a name \"holon\" hasn't": `import { flwo } from "holon"`,
    "a name \"holon\" keeps to itself": `import { answer } from "holon"`,
  };

  it("catches the mistakes it should, each on its line", () => {
    const flows = Object.fromEntries(Object.values(mistakes).map((line, i) => [`/mistakes/${i}.holon.ts`, `${line}\n${head}`]));
    const found = check(flows);
    const lines = Object.keys(mistakes).map((what, i) => [
      what,
      (found.get(`/mistakes/${i}.holon.ts`) ?? []).map((d) => d.file!.getLineAndCharacterOfPosition(d.start!).line + 1),
    ]);
    expect(lines).toEqual(Object.keys(mistakes).map((what) => [what, [1]]));
  });

  it("lets Holon's differences through, and only them", () => {
    const flow = `${head}route.post("/x", (req) => ({ total: req.json.total } ?? status(400)))\n`;
    const found = check({ "/differences/0.holon.ts": flow });
    expect([...found.values()].flat().map((d) => d.code)).toEqual([...DIFFERENCES]);
  });
});
