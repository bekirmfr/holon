// The agent surface (docs/L4s/agent-surface-L4.md), through the real command, server and editor:
// an agent writes a flow and deploys it with `holon mcp`, which the server holds to proposing (the
// agent role, access.rs) though it signs in as the person; it waits for them, and they deploy it
// from the flows page, approving what it may do; after that the agent's next version deploys
// at once, and the agent calls it and reads the run back as its own text.

import { spawn } from "node:child_process";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { expect, PORTS, RUN_DIR, SAMPLE_SHIFT, SERVER, test } from "./fixtures";

/** `holon mcp` against the test server, as an agent's host runs it: one JSON-RPC line each way. */
function mcp() {
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const child = spawn(holon, ["mcp", "--server", String(PORTS.server)], { env: { ...process.env, HOLON_RUN_DIR: RUN_DIR, HOLON_TOKEN: "", HOLON_SERVER: "" } });
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

test("an agent proposes a flow through holon mcp, a person approves it in the editor, and the agent runs and explains it", async ({ page }) => {
  const agent = mcp();
  try {
    const init = await agent.rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } });
    expect(init.result?.protocolVersion).toBe("2025-06-18");
    // Its own name each run, so a reused server's earlier runs don't meet it; its port past the
    // samples', in this slot's range (ports.mjs), so it meets no other run's.
    const name = `agent-${Date.now().toString(36)}`;
    const port = 8190 + SAMPLE_SHIFT;
    const text = (greeting: string) => `flow ${name}\nport ${port}\n\nroute GET /hi/:name (req) {\n  respond "${greeting}, {req.params.name}"\n}\n`;

    expect((await agent.tool("check", { text: text("Hi") })).text).toContain("GET /hi/:name");
    // The first deploy waits for a person.
    const proposed = await agent.tool("deploy", { text: text("Hi") });
    expect(proposed.failed).toBe(false);
    expect(proposed.text).toContain("Not deployed yet");
    expect(proposed.text).toContain("receive requests");

    // The person: the flows page lists it, saved by the agent under their sign-in; they open it and deploy it, approving.
    await page.goto("/");
    await page.getByRole("button", { name: "flows", exact: true }).click();
    const row = page.getByText(name, { exact: true }).locator("xpath=..");
    await expect(row).toContainText(/by local:.+ \(agent\)/);
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
    expect(explained.text).toContain('respond "Hello, {req.params.name}"');
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
    const port = 8191 + SAMPLE_SHIFT;
    const text = `flow ${name}\nport ${port}\n\nroute POST /add (req) {\n  total = (state.total ?? 0) + req.json.n\n  state.total = total\n  respond total\n}\n\nroute GET /total (req) {\n  respond state.total\n}\n`;
    expect((await agent.tool("deploy", { text })).text).toContain("Not deployed yet");
    // A person deploys it, approving what it may do.
    await page.goto("/");
    await page.getByRole("button", { name: "flows", exact: true }).click();
    await page.getByText(name, { exact: true }).locator("xpath=..").getByRole("button", { name: "open", exact: true }).click();
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
    await page.locator(".hl-canvas").getByRole("button", { name: "view", exact: true }).dispatchEvent("click");
    const bottom = page.locator(".hl-view__bottom");
    await bottom.getByRole("button", { name: "history", exact: true }).click();
    await bottom.locator(".hl-history__row").filter({ hasText: `asks to put back the state run ${first} left` }).first().click();
    await expect(bottom.locator(".hl-history__details")).toContainText(`run ${first}'s state, for a person to restore`);
    await expect(bottom.locator(".hl-history__details")).toContainText("(agent)");
    const restoreBar = bottom.locator(".hl-history__restore");
    await expect(restoreBar.getByRole("button", { name: "code", exact: true })).toBeDisabled();
    await restoreBar.getByRole("button", { name: "state", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "dry run" });
    await expect(dialog).toContainText(`The state run ${first} left is put back`);
    await dialog.getByRole("button", { name: "Restore", exact: true }).click();
    await expect(bottom.locator(".hl-history__note")).toContainText(`the state run ${first} left is live again`);
    expect(await total()).toBe(5);
  } finally {
    agent.close();
  }
});
