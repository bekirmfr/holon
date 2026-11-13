// The agent surface (docs/L4s/agent-surface-L4.md), through the real command, server and editor:
// an agent writes a flow and deploys it with `holon mcp`, which signs in as the OS user's agent
// (made for them through the run file the first time, mcphosts.rs), so the server holds it to
// proposing (the agent role, access.rs); it waits for a person, and they deploy it from the flows
// page, approving what it may do; after that the agent's next version deploys at once, and the
// agent calls it and reads the run back as its own text. And the proposal sample (proposals.rs):
// proposed with a why and a note, declined in Approvals with a reason the agent reads back,
// proposed again and approved from its review; then agents' proposals turned off.

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import type { APIRequestContext } from "@playwright/test";
import { expect, OWN_PORTS, PORTS, RUN_DIR, SAMPLE_SHIFT, SERVER, test } from "./fixtures";
import { flowRow, gotoPage } from "./helpers";

/** The agent `holon mcp` made for the OS user here: the one with the tokens it made (mcphosts.rs). */
async function mcpAgent(request: APIRequestContext, adminToken: string) {
  const people = (await (await request.get(`${SERVER}/people`, { headers: { Authorization: `Bearer ${adminToken}` } })).json()) as { name: string; kind: string; tokens?: { label: string }[] }[];
  const made = people.filter((p) => p.kind === "agent" && (p.tokens ?? []).some((t) => t.label.startsWith("holon mcp on ")));
  expect(made.map((p) => p.name)).toHaveLength(1);
  return made[0].name;
}

/** `holon mcp` against the test server, as an agent's host runs it: one JSON-RPC line each way.
 *  Its home is a new folder, so it makes its agent's token there through the run file, as it
 *  does the first time, and never keeps one in the user's own. */
function mcp() {
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const home = mkdtempSync(join(tmpdir(), "holon-e2e-mcp-"));
  const child = spawn(holon, ["mcp", "--server", String(PORTS.server)], { env: { ...process.env, HOLON_HOME: home, HOLON_RUN_DIR: RUN_DIR, HOLON_TOKEN: "", HOLON_SERVER: "" } });
  const waiting = new Map<number, (v: { result?: { content: { text: string }[]; isError: boolean }; error?: unknown }) => void>();
  createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    waiting.get(message.id)?.(message);
  });
  let id = 0;
  const rpc = (method: string, params: object) =>
    new Promise<{ result?: { content: { text: string }[]; isError: boolean; protocolVersion?: string }; error?: unknown }>((resolve) => {
      id += 1;
      waiting.set(id, resolve);
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  /** A tool's answer, and whether it failed. */
  const tool = async (name: string, args: object) => {
    const r = await rpc("tools/call", { name, arguments: args });
    return { text: r.result!.content[0].text, failed: r.result!.isError };
  };
  return { rpc, tool, close: () => child.kill() };
}

test("an agent proposes a flow through holon mcp, a person approves it in the editor, and the agent runs and explains it", async ({ page, request, adminToken }) => {
  const agent = mcp();
  try {
    const init = await agent.rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } });
    expect(init.result?.protocolVersion).toBe("2025-06-18");
    // Its own name each run, so a reused server's earlier runs don't meet it; its port one no
    // sample takes, in this slot's range (ports.mjs), so it meets no other run's.
    const name = `agent-${Date.now().toString(36)}`;
    const port = OWN_PORTS.greeting + SAMPLE_SHIFT;
    const text = (greeting: string) => `import { flow, route } from "holon"\n\nflow("${name}", { port: ${port} })\n\nroute.get("/hi/:name", (req) => \`${greeting}, \${req.params.name}\`)\n`;

    expect((await agent.tool("check", { text: text("Hi") })).text).toContain("GET /hi/:name");
    // The first deploy waits for a person.
    const proposed = await agent.tool("deploy", { text: text("Hi") });
    expect(proposed.failed).toBe(false);
    expect(proposed.text).toContain("Not deployed yet");
    expect(proposed.text).toContain("receive requests");

    // The person: the flows page lists it, saved by the agent under its own name; they open it and deploy it, approving.
    await page.goto("/");
    await gotoPage(page, "flows");
    const row = flowRow(page, name);
    await expect(row).toContainText(`by ${await mcpAgent(request, adminToken)}`);
    await row.getByRole("button", { name: "open", exact: true }).click();
    await expect(page.locator(".hl-tab--active")).toContainText(name);
    await page.locator(".hl-canvas").getByRole("button", { name: /^deploy/ }).click();
    const consent = page.getByRole("dialog");
    await expect(consent).toContainText("receive requests");
    await consent.getByRole("button", { name: /^(Approve and deploy|Deploy)$/ }).click();
    await expect(page.locator(".hl-tab--active")).toContainText("deployed");

    // Now a version that needs nothing more deploys from the agent at once.
    const updated = await agent.tool("deploy", { text: text("Hello") });
    expect(updated.text).toContain(`Deployed "${name}"`);
    const called = await agent.tool("call", { slug: name, method: "GET", path: "/hi/Ada" });
    expect(called.text).toContain("GET /hi/Ada → 200");
    expect(called.text).toContain("Hello, Ada");
    const explained = await agent.tool("explain", { slug: name });
    expect(explained.text).toContain("`Hello, ${req.params.name}`");
    expect(explained.text).toContain('⇒ { body: "Hello, Ada", status: 200 }');
  } finally {
    agent.close();
  }
});

test("an agent asks through holon mcp to put a run's state back, and a person restores it from the history", async ({ page, request, adminToken }) => {
  const agent = mcp();
  try {
    await agent.rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } });
    const name = `tally-${Date.now().toString(36)}`;
    const port = OWN_PORTS.total + SAMPLE_SHIFT;
    const text = `import { flow, route } from "holon"\nimport state from "state"\n\nflow("${name}", { port: ${port} })\n\nroute.post("/add", (req) => {\n  const total = (state.total ?? 0) + req.json.n\n  state.total = total\n  return total\n})\n\nroute.get("/total", () => state.total)\n`;
    expect((await agent.tool("deploy", { text })).text).toContain("Not deployed yet");
    // A person deploys it, approving what it may do.
    await page.goto("/");
    await gotoPage(page, "flows");
    await flowRow(page, name).getByRole("button", { name: "open", exact: true }).click();
    // Opened once its tab says so: deploy pressed before that deploys the untitled draft, and asks
    // for a name instead of what the flow may do.
    await expect(page.locator(".hl-tab--active")).toContainText(name);
    await page.locator(".hl-canvas").getByRole("button", { name: /^deploy/ }).dispatchEvent("click");
    await page.getByRole("dialog").getByRole("button", { name: /^(Approve and deploy|Deploy)$/ }).click();
    await expect(page.locator(".hl-tab--active")).toContainText("deployed");

    // 5, then a mistaken 100.
    const add = async (n: number) => (await request.post(`http://127.0.0.1:${port}/add`, { data: { n } })).json();
    const total = async () => (await request.get(`http://127.0.0.1:${port}/total`)).json();
    expect(await add(5)).toBe(5);
    expect(await add(100)).toBe(105);
    const log = await (await request.get(`${SERVER}/deployments/${name}/log`, { headers: { Authorization: `Bearer ${adminToken}` } })).json();
    const first = (log.entries as { seq: number }[])[0].seq;

    // The agent asks to put back the state the first run left: saved for a person, nothing changed.
    const asked = await agent.tool("restore", { slug: name, run: first, state: true, ask: true });
    expect(asked.failed).toBe(false);
    expect(asked.text).toContain("total: 105 → 5");
    expect(asked.text).toContain(`Saved for a person to approve (event #`);
    expect(asked.text).toContain(`it asks to put back the state run ${first} left`);
    expect(await total()).toBe(105);

    // The person finds it in the history, its details saying who asked, and restores it.
    await page.goto(`/#${name}.history`);
    const bottom = page.getByRole("region", { name: "history" });
    await bottom.locator(".hl-history__row").filter({ hasText: `asks to put back the state run ${first} left` }).first().click();
    await expect(bottom.locator(".hl-history__details")).toContainText(`run ${first}'s state, for a person to restore`);
    await expect(bottom.locator(".hl-history__details")).toContainText(await mcpAgent(request, adminToken));
    const goBack = bottom.getByRole("region", { name: /^go back to / });
    await expect(goBack.getByRole("radio", { name: "Code only" })).toBeDisabled();
    await goBack.getByRole("radio", { name: "State only" }).click();
    await goBack.getByRole("button", { name: "Go back…" }).click();
    const dialog = page.getByRole("dialog", { name: "dry run" });
    await expect(dialog).toContainText(`The state run ${first} left is put back`);
    await dialog.getByRole("button", { name: "Restore", exact: true }).click();
    await expect(bottom.locator(".hl-history__note")).toContainText(`the state run ${first} left is live again`);
    expect(await total()).toBe(5);
  } finally {
    agent.close();
  }
});

test("an agent proposes the proposal sample with a why; a person declines it with a reason the agent reads, then approves it again proposed", async ({ page, request, adminToken }) => {
  const agent = mcp();
  const slug = `proposal-${Date.now().toString(36)}`;
  const headers = { Authorization: `Bearer ${adminToken}` };
  try {
    await agent.rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } });
    // Its port one no sample takes, in this slot's range, so the sample deployed by samples.e2e meets it not.
    const text = readFileSync(new URL("../../crates/holon-author/samples/proposal.holon.ts", import.meta.url), "utf8").replace("port: 8184", `port: ${OWN_PORTS.proposal + SAMPLE_SHIFT}`);
    const why = "ids from httpbin, so they are unique across hosts";
    const proposed = await agent.tool("deploy", { text, slug, why, note: "asked for in #payments; tried against httpbin: 3 ids" });
    expect(proposed.failed).toBe(false);
    expect(proposed.text).toContain(`proposed as version 1 of "${slug}"`);
    expect(proposed.text).toContain(`#approvals.${slug}.v1`);

    // Approvals shows it as a card with the agent's why and note, and the rows it asks to approve,
    // each with its line; declined, with a reason, it leaves.
    await page.goto("/#approvals");
    const row = page.getByRole("article", { name: `proposal ${slug}` });
    await expect(row).toContainText(why);
    await expect(row).toContainText("asked for in #payments");
    await expect(row).toContainText("new, v1");
    const asks = row.getByRole("list", { name: "what changes" }).getByRole("listitem");
    await expect(asks).toHaveCount(2);
    await expect(asks.filter({ hasText: "host httpbin.org: call" })).toContainText(/line \d+/);
    await expect(row).toContainText("0 the same");
    // At phone width the list still fits across, the button that says it asks why with it.
    const wide = page.viewportSize()!;
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(row.getByRole("button", { name: `decline ${slug}` })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.setViewportSize(wide);
    await expect(row.getByRole("button", { name: `decline ${slug}` })).toHaveText("Decline with a reason…");
    await row.getByRole("button", { name: `decline ${slug}` }).click();
    const dialog = page.locator('[aria-modal="true"]');
    await expect(dialog).toContainText("Why it was declined");
    await expect(dialog).toContainText("the agent reads it through holon mcp");
    await dialog.locator("input").fill("use our own id service, not httpbin");
    await dialog.getByRole("button", { name: "Submit" }).click();
    await expect(page.getByText(`Declined ${slug} v1. The agent gets your note.`)).toBeVisible();
    await expect(row).toHaveCount(0);
    // The agent reads why.
    const read = await agent.tool("deployments", { slug });
    expect(read.text).toMatch(/proposal \d+ was declined by e2e at .*: use our own id service, not httpbin/);

    // Proposed again, it is reviewed as proposed: its why, its note, and what approving it does.
    expect((await agent.tool("deploy", { text, slug, why: "ids from httpbin, for now" })).text).toContain("proposed as version 1");
    await page.goto("/#approvals");
    await page.getByRole("link", { name: `review ${slug}` }).click();
    await expect(page).toHaveURL(new RegExp(`#approvals\\.${slug}\\.v1$`));
    const decision = page.getByRole("complementary", { name: "decision" });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(decision.getByRole("button", { name: "Decline with a reason…" })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.setViewportSize(wide);
    await expect(decision.getByRole("group", { name: "proposal" })).toContainText("Why: ids from httpbin, for now");
    await expect(decision.getByRole("group", { name: "if approved" })).toContainText("v1 goes live.");
    await expect(decision.getByRole("group", { name: "if approved" })).toContainText("You grant 2 new permissions");
    await decision.getByRole("checkbox").check();
    await decision.getByRole("button", { name: "Approve and deploy" }).click();
    await expect(page.getByText(`Deployed ${slug} v1.`)).toBeVisible();
    const proposals = (await (await request.get(`${SERVER}/proposals`, { headers })).json()).proposals as { slug: string; state: string; decided_by?: string }[];
    expect(proposals.filter((p) => p.slug === slug).map((p) => p.state)).toEqual(["approved", "declined"]);

    // Its settings turn agents' proposals off: the agent's next deploy is refused.
    await page.goto(`/#${slug}.settings`);
    const agents = page.getByRole("region", { name: "agents" });
    await agents.getByLabel("agents may propose versions").uncheck();
    await expect(agents).toContainText("An agent's deploy of it is refused.");
    const refused = await agent.tool("deploy", { text: text.replace("as the id", "as its id"), slug });
    expect(refused.failed).toBe(true);
    expect(refused.text).toContain("turned off agents proposing versions");
  } finally {
    agent.close();
    await request.post(`${SERVER}/deployments/${slug}/stop`, { headers });
    await request.delete(`${SERVER}/deployments/${slug}`, { headers });
    await request.delete(`${SERVER}/flows/${slug}`, { headers });
  }
});
