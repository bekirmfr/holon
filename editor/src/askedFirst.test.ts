// Guards: every call of the console that can't be taken back, or that ends or changes what someone
// may do (a removal, an unbinding, a token revoked, a person disabled, a role or an owner changed),
// is asked first: a question awaited before it in a handler it runs in (a confirm, a prompt for its
// reason, a dry run's review), or ASKED_ELSEWHERE says where it is asked instead. Needed because one
// click on a row's × removed a flow's secret, every version; one on "disable" ended someone's
// sign-ins; an arrow key on a focused role list changed their role (R43). If it fails, the call it
// names runs on one click: ask before it, or say in ASKED_ELSEWHERE where it is asked.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const dir = join(process.cwd(), "src");
const parse = (file: string) => ts.createSourceFile(file, readFileSync(join(dir, file), "utf8"), ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);

/** The calls asked first: every function of server.ts that sends a DELETE, and these, which end or
 *  change what someone may do without one. */
const CHANGES_WHO_MAY = ["changePrincipal"];

/** Calls asked somewhere other than the handler that makes them: where, and how. */
const ASKED_ELSEWHERE: Record<string, string> = {
  "App.tsx deleteDeployment": "FlowSettings' Remove: the flow's name typed, the button disabled until it is, and the flow stopped first",
};

/** The questions a handler awaits: a dialog's confirm, prompt or choice, a dry run's review. */
const QUESTIONS = new Set(["confirm", "prompt", "choose", "ask"]);

function askedFirst(): { calls: string[]; unasked: string[] } {
  const server = parse("server.ts");
  const deletes: string[] = [];
  server.forEachChild((node) => {
    if (ts.isFunctionDeclaration(node) && node.name && node.body && /method: "DELETE"/.test(node.body.getText())) deletes.push(node.name.text);
  });
  const named = new Set([...deletes, ...CHANGES_WHO_MAY]);
  const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f) && f !== "server.ts");
  const calls: string[] = [];
  const unasked: string[] = [];
  for (const file of files) {
    const source = parse(file);
    const visit = (node: ts.Node) => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && named.has(node.expression.text)) {
        const where = `${file.replace(/\\/g, "/")} ${node.expression.text}`;
        const line = source.getLineAndCharacterOfPosition(node.getStart()).line + 1;
        calls.push(where);
        if (!ASKED_ELSEWHERE[where] && !questionBefore(node)) unasked.push(`${file.replace(/\\/g, "/")}:${line} ${node.expression.text}`);
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return { calls, unasked };
}

/** The function a node is in, nearest first. */
function functionsAround(node: ts.Node): ts.Node[] {
  const around: ts.Node[] = [];
  for (let at: ts.Node | undefined = node.parent; at; at = at.parent) if (ts.isFunctionLike(at)) around.push(at);
  return around;
}

/** Whether a function the call runs in awaits a question before it, itself and not in a handler
 *  of its own: a component whose other button asks first doesn't ask for this one. */
function questionBefore(call: ts.Node): boolean {
  const around = functionsAround(call);
  let found = false;
  const look = (node: ts.Node) => {
    if (found || node.getStart() >= call.getStart()) return;
    if (ts.isAwaitExpression(node)) {
      const asked = node.expression;
      const question = ts.isCallExpression(asked) && ts.isPropertyAccessExpression(asked.expression) && QUESTIONS.has(asked.expression.name.text);
      if (question && around.includes(functionsAround(node)[0])) found = true;
    }
    ts.forEachChild(node, look);
  };
  for (const f of around) look(f);
  return found;
}

describe("a call that can't be taken back, or changes what someone may do", () => {
  it("is asked first", () => {
    const { calls, unasked } = askedFirst();
    // The sweep finds them: the removals of the People page, the secrets', the flows'.
    for (const c of ["PeoplePages.tsx removePrincipal", "PeoplePages.tsx changePrincipal", "DeploySettings.tsx deleteSecret", "FlowsPage.tsx deleteFlow", "HostPages.tsx unbindServerSecret"]) expect(calls).toContain(c);
    expect(unasked).toEqual([]);
  });

  it("names in ASKED_ELSEWHERE only calls the console makes", () => {
    const { calls } = askedFirst();
    for (const where of Object.keys(ASKED_ELSEWHERE)) expect(calls, where).toContain(where);
  });
});
