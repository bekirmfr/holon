// Guards: a list whose read failed is never shown as an empty one. Each place that lists what
// waits, what was done or what is set says it couldn't read it, and why, where it would have said
// "nothing waits", "No run is waiting.", "not set" or "nothing yet"; and no handler of a failed
// read in the console sets a list to none (the sweep below), save the EXEMPT, which say why
// none is the truth there. Needed because a failed read of the runs waiting, the access changes or
// a flow's secrets showed "No run is waiting.", "No access change waits." and "not set" to whoever
// was to decide them (R44). If it fails: the sweep names the handler that empties a list on a
// failed read (keep why, and say it with ui/Unread.tsx); a row names the page that shows a list
// not read as empty.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import ts from "typescript";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { ApprovalsPage } from "./ApprovalsPage";
import { resetCan } from "./can";
import { SecretRow } from "./DeploySettings";
import { DialogProvider } from "./Dialog";
import { Waiting } from "./FlowSettings";
import { OverviewPage } from "./OverviewPage";
import { ActivityList } from "./Pages";
import { getActivity, listAccessChanges, listAllWaitingRuns, listFlows, listProposals, listWaitingRuns, ServerError } from "./server";
import { useApprovals, type Unread } from "./useApprovals";

vi.mock("./server", async (original) => {
  const real = await original<typeof import("./server")>();
  return {
    ...real,
    mayI: vi.fn(async () => ({})),
    whoAmI: vi.fn(async () => ({ name: "ada", kind: "person", role: "admin", access_version: 1 })),
    getSummary: vi.fn(() => new Promise(() => {})),
    getWarnings: vi.fn(async () => []),
    getActivity: vi.fn(),
    listWaitingRuns: vi.fn(),
    listAllWaitingRuns: vi.fn(),
    listFlows: vi.fn(),
    listProposals: vi.fn(),
    listAccessChanges: vi.fn(),
  };
});

const dir = join(process.cwd(), "src");
const parse = (file: string) => ts.createSourceFile(file, readFileSync(join(dir, file), "utf8"), ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);

/** Handlers of a failed read that set none, and why none is the truth there. */
const EXEMPT: Record<string, string> = {
  "CellHistory.tsx setVersions": "the versions only label each change with the version it was under; the history itself says why it couldn't be read",
  "DeploySettings.tsx setEnvNames": "the server's env variables' names are suggestions: with none, a name is typed",
  "FlowSettings.tsx setEnvNames": "the server's env variables' names are suggestions: with none, a name is typed",
};

/** What says a list is empty: `[]`, a new empty Map or Set, or `(was) => was ?? []`. (`null` is
 *  how the console says a thing isn't read yet.) */
function empty(e: ts.Expression): boolean {
  if (ts.isArrayLiteralExpression(e)) return e.elements.length === 0;
  if (ts.isNewExpression(e)) return ts.isIdentifier(e.expression) && /^(Map|Set)$/.test(e.expression.text) && (e.arguments?.length ?? 0) === 0;
  if (ts.isArrowFunction(e) && !ts.isBlock(e.body)) {
    const b = e.body;
    return ts.isBinaryExpression(b) && b.operatorToken.kind === ts.SyntaxKind.QuestionQuestionToken && empty(b.right);
  }
  return false;
}

/** The state setters a failed read's handler calls with none, whatever went wrong: its body's
 *  own calls, not those under a condition (a refusal that means none, say). */
function emptied(handler: ts.Node): string[] {
  if (!ts.isArrowFunction(handler) && !ts.isFunctionExpression(handler)) return [];
  const calls = ts.isBlock(handler.body) ? handler.body.statements.flatMap((s) => (ts.isExpressionStatement(s) ? [s.expression] : [])) : [handler.body];
  return calls.flatMap((c) => (ts.isCallExpression(c) && ts.isIdentifier(c.expression) && /^set[A-Z]/.test(c.expression.text) && c.arguments.length === 1 && empty(c.arguments[0]) ? [c.expression.text] : []));
}

/** Every handler of a failed read (`.then(ok, failed)`, `.catch(failed)`) that sets a list to none. */
function emptiedOnFailure(): string[] {
  const files = readdirSync(dir, { recursive: true, encoding: "utf8" }).filter((f) => /\.tsx?$/.test(f) && !/\.test\.tsx?$/.test(f));
  const found: string[] = [];
  for (const file of files) {
    const source = parse(file);
    const visit = (node: ts.Node) => {
      if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)) {
        const method = node.expression.name.text;
        const failed = method === "then" ? node.arguments[1] : method === "catch" ? node.arguments[0] : undefined;
        if (failed) for (const setter of emptied(failed)) found.push(`${file.replace(/\\/g, "/")} ${setter}`);
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return found;
}

describe("a failed read, in the console's source", () => {
  it("never sets a list to none, save where none is the truth", () => {
    const found = emptiedOnFailure();
    expect(found.filter((f) => !EXEMPT[f])).toEqual([]);
    // Each exemption is still there: one not found is out of date.
    expect(Object.keys(EXEMPT).filter((e) => !found.includes(e))).toEqual([]);
  });
});

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});
let container: HTMLDivElement | null = null;
let root: Root | null = null;
beforeEach(() => {
  resetCan();
  vi.mocked(getActivity).mockResolvedValue([]);
});
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});
async function show(node: ReactNode) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => root!.render(<DialogProvider armMs={0}>{node}</DialogProvider>));
  for (let i = 0; i < 3; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
}
const text = () => container!.textContent ?? "";
const alerted = () => [...container!.querySelectorAll('[role="alert"]')].map((a) => a.textContent ?? "");
/** Where it says it couldn't read a list: an alert, or a row's status and its title. */
const said = () => [...alerted(), ...[...container!.querySelectorAll("[title]")].map((t) => `${t.textContent}: ${t.getAttribute("title")}`)].join(" ");

const WHY = "the server isn't answering (TypeError: Failed to fetch)";
const none: Unread = { deploys: "", runs: "", access: "" };
const me = { name: "ada", kind: "person" as const, role: "admin" as const };
const approvals = (tab: "deploys" | "runs" | "access", unread: Unread) => (
  <ApprovalsPage tab={tab} approvals={[]} deployments={[]} runs={[]} access={[]} unread={unread} me={me} navigate={() => {}} textOf={() => ""} onDeclined={() => {}} onDecided={() => {}} />
);

/** Each place that lists something, its list not read: what it says when read and empty, and how
 *  it is shown with its read failed. */
const PLACES: { name: string; empty: string; render: (unread: boolean) => Promise<ReactNode> | ReactNode }[] = [
  { name: "Approvals' Deploys", empty: "Nothing waits for you", render: (u) => approvals("deploys", { ...none, deploys: u ? WHY : "" }) },
  { name: "Approvals' Runs waiting", empty: "No run is waiting.", render: (u) => approvals("runs", { ...none, runs: u ? WHY : "" }) },
  { name: "Approvals' Access changes", empty: "No access change waits.", render: (u) => approvals("access", { ...none, access: u ? WHY : "" }) },
  {
    name: "the overview",
    empty: "Nothing waits for approval.",
    render: (u) => <OverviewPage name="ada" admin={false} deployments={[]} approvals={[]} runs={[]} unread={{ ...none, deploys: u ? WHY : "" }} navigate={() => {}} onNew={() => {}} />,
  },
  {
    name: "a flow's waiting runs",
    empty: "Waiting runs",
    render: (u) => {
      vi.mocked(listWaitingRuns).mockImplementation(u ? async () => Promise.reject(new TypeError("Failed to fetch")) : async () => []);
      return <Waiting slug="refunds" count={1} />;
    },
  },
  {
    name: "a flow's secret",
    empty: "not set",
    render: (u) => <SecretRow slug="refunds" name="stripe" unread={u ? WHY : ""} envNames={[]} onChanged={() => {}} />,
  },
  {
    name: "what was done on the server",
    empty: "nothing yet",
    render: (u) => {
      vi.mocked(getActivity).mockImplementation(u ? async () => Promise.reject(new TypeError("Failed to fetch")) : async () => []);
      return <ActivityList version="1" bare />;
    },
  },
];

describe("a list that couldn't be read", () => {
  it.each(PLACES.map((p) => [p.name, p] as const))("%s: says it couldn't, and why, and never that it is empty", async (_, place) => {
    await show(await place.render(false));
    expect(text(), "read and empty, it says so").toContain(place.empty);
    expect(alerted()).toEqual([]);
    act(() => root?.unmount());
    container?.remove();
    await show(await place.render(true));
    expect(said(), "it says it couldn't read it, and why").toMatch(new RegExp(`couldn't (read|be read).*: ${WHY.replace(/[()]/g, "\\$&")}`, "i"));
    if (place.empty !== "Waiting runs") expect(text(), "never that it is empty").not.toContain(place.empty);
  });

  it("Approvals' tab counts say none is known, not 0", async () => {
    await show(approvals("deploys", { deploys: WHY, runs: WHY, access: "" }));
    const counts = [...container!.querySelectorAll('nav[aria-label="approvals tabs"] .hl-flow__count')].map((c) => c.textContent);
    expect(counts).toEqual(["?", "?", "0"]);
  });
});

/** Reads what waits through useApprovals, as App does. */
function Reads({ out }: { out: (u: ReturnType<typeof useApprovals>) => void }) {
  out(useApprovals([], { shown: true, asks: false }));
  return null;
}

describe("useApprovals, its reads failing", () => {
  it("keeps each list's why until a read of it succeeds; a refused read of access changes is none", async () => {
    let got: ReturnType<typeof useApprovals> | null = null;
    vi.mocked(listFlows).mockRejectedValue(new ServerError(500, "the flows store is locked"));
    vi.mocked(listProposals).mockResolvedValue([]);
    vi.mocked(listAllWaitingRuns).mockRejectedValue(new TypeError("Failed to fetch"));
    vi.mocked(listAccessChanges).mockRejectedValue(new ServerError(403, "ada may not do that"));
    await show(<Reads out={(u) => (got = u)} />);
    expect(got!.unread).toEqual({ deploys: "the flows store is locked", runs: WHY, access: "" });
    expect(got!.runs, "not read: not none").toBeNull();
    expect(got!.access, "refused: none to see").toEqual([]);
    vi.mocked(listFlows).mockResolvedValue([]);
    vi.mocked(listAllWaitingRuns).mockResolvedValue([]);
    await act(async () => got!.refresh());
    for (let i = 0; i < 3; i++) await act(async () => new Promise((r) => setTimeout(r, 0)));
    expect(got!.unread).toEqual(none);
    expect(got!.runs).toEqual([]);
  });
});
