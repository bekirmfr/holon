// Getting started's commands (guide.ts; web-console-L4.md §10): every one is a command the `holon`
// binary takes, read from its own source (crates/holon/src): the command is one main() dispatches,
// its words before any option are the ones its line in the usage asks for (a file, a slug, a
// subcommand), each option is one the source parses, and a sample it names is one there is. The
// Library's `holon new` line for a sample is held to the same.

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { START_STEPS } from "./guide";
import { newCommand } from "./library";
import { samples } from "./samples";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

const SRC = join(process.cwd(), "..", "crates", "holon", "src");
const MAIN = readFileSync(join(SRC, "main.rs"), "utf8");
/** Every Rust file of the binary, as one text: where an option is parsed, it is a string. */
const SOURCE = readdirSync(SRC, { recursive: true })
  .map(String)
  .filter((f) => f.endsWith(".rs"))
  .map((f) => readFileSync(join(SRC, f), "utf8"))
  .join("\n");

/** The commands main() dispatches: `Some("deploy") =>`, `Some(cmd @ ("new" | "check")) =>`. */
function dispatched(): Set<string> {
  const body = MAIN.slice(MAIN.indexOf("fn main()"), MAIN.indexOf("fn serve("));
  const out = new Set<string>();
  for (const arm of body.matchAll(/Some\((.*?)\)\s*=>/g)) for (const name of arm[1].matchAll(/"([a-z-]+)"/g)) out.add(name[1]);
  return out;
}

/** The usage's lines, by command: each the words after its name (`new <file> [--sample <name>]`
 *  is new's `["<file>", "[--sample", "<name>]"]`). A command may have several (`mcp`). */
function usage(): Map<string, string[][]> {
  const text = /const USAGE: &str = "([\s\S]*?)";/.exec(MAIN)![1];
  const out = new Map<string, string[][]>();
  for (const l of text.split("\n")) {
    const left = /^ {2}(\S.*?)(?: {2,}|$)/.exec(l.replace(/\s*\|\s*/g, "|"))?.[1];
    if (!left) continue;
    const [names, ...words] = left.split(/\s+/);
    for (const name of names.split("|")) out.set(name, [...(out.get(name) ?? []), words]);
  }
  return out;
}

/** Why `line` isn't a command `holon` takes, or null. */
function refusal(line: string, commands: Set<string>, shapes: Map<string, string[][]>): string | null {
  const [bin, cmd, ...args] = line.split(/\s+/);
  if (bin !== "holon") return `${line}: not the holon command`;
  if (!commands.has(cmd)) return `${line}: holon has no command ${cmd}`;
  for (const flag of args.filter((a) => a.startsWith("-"))) if (!SOURCE.includes(`"${flag}"`)) return `${line}: holon ${cmd} parses no ${flag}`;
  const forms = shapes.get(cmd);
  // A command the usage doesn't list (`pick-folder`) is held to its dispatch and its options alone.
  if (!forms) return null;
  // The words before the first option, each the form's word in its place: a placeholder
  // (`<file>`, `METHOD`, `HOST…` and every word after it) takes any, a subcommand only its own.
  const flag = args.findIndex((a) => a.startsWith("-"));
  const lead = flag < 0 ? args : args.slice(0, flag);
  const fits = (form: string[]) => {
    const optional = form.findIndex((w) => w.startsWith("["));
    if (lead.length < (optional < 0 ? form.length : optional)) return false;
    const words = form.map((w) => w.replace(/[[\]]/g, ""));
    let at = 0;
    return lead.every((a) => {
      const w = words[at];
      if (w === undefined || w.startsWith("-") || !w.split("|").some((alt) => /^<.*>$|^[A-Z]+…?$/.test(alt) || alt === a)) return false;
      if (!w.includes("…")) at++;
      return true;
    });
  };
  return forms.some(fits) ? null : `${line}: holon ${cmd} takes ${forms.map((f) => f.join(" ")).join(", or ")}`;
}

describe("Getting started's commands", () => {
  const commands = dispatched();
  const shapes = usage();
  const lines = START_STEPS.flatMap((s) => s.commands.map((c) => c.line));

  it("read the binary's commands and usage", () => {
    for (const c of ["serve", "new", "check", "deploy", "call", "runs", "mcp"]) expect(commands).toContain(c);
    expect(shapes.get("new")).toEqual([["<file>", "[--sample", "<name>]"]]);
    expect(shapes.get("runs")).toEqual([["<slug>", "[--last", "N]"]]);
    // Every command main() takes has its line, but the one only the server runs.
    expect([...commands].filter((c) => !shapes.has(c) && c !== "pick-folder")).toEqual([]);
    expect(shapes.get("mcp")).toContainEqual(["setup", "[HOST…|--all]"]);
    expect(shapes.get("policy")).toBeDefined();
  });

  it("refuse what holon doesn't take", () => {
    expect(refusal("holon new --sample hello-api", commands, shapes)).toMatch(/takes <file>/);
    expect(refusal("holon deploy x.holon.ts --yes-please", commands, shapes)).toMatch(/parses no --yes-please/);
    expect(refusal("holon launch x", commands, shapes)).toMatch(/no command launch/);
    expect(refusal("holon mcp install claude-code", commands, shapes)).toMatch(/takes/);
    expect(refusal("holon call hello-api GET", commands, shapes)).toMatch(/takes <slug> METHOD PATH/);
  });

  it("are each one holon takes, with what it does", () => {
    expect(lines.length).toBeGreaterThan(0);
    for (const s of START_STEPS) for (const c of s.commands) expect(c.does.trim(), c.line).not.toBe("");
    expect(lines.map((l) => refusal(l, commands, shapes)).filter((r) => r !== null)).toEqual([]);
  });

  it("name samples there are, and the Library's line for each sample is one holon takes", () => {
    const names = new Set(samples().map((s) => s.name));
    for (const l of lines) for (const m of l.matchAll(/--sample (\S+)/g)) expect(names, l).toContain(m[1]);
    expect(lines).toContain(newCommand({ name: "hello-api" }));
    expect(samples().map((s) => refusal(newCommand(s), commands, shapes)).filter((r) => r !== null)).toEqual([]);
  });
});
