// The agent surface (docs/L4s/agent-surface-L4.md), through the real command, server and editor:
// an agent writes a flow and deploys it with `holon mcp`; it waits for a person, who deploys it
// from the flows page and approves what it may do; after that the agent's next version deploys
// at once, and the agent calls it and reads the run back as its own text.

import { spawn } from "node:child_process";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { expect, RUN_DIR, test } from "./fixtures";

/** `holon mcp` against the test server, as an agent's host runs it: one JSON-RPC line each way. */
function mcp() {
  const target = process.env.CARGO_TARGET_DIR ?? join(process.cwd(), "..", "target");
  const holon = join(target, "debug", process.platform === "win32" ? "holon.exe" : "holon");
  const child = spawn(holon, ["mcp", "--server", "7520"], { env: { ...process.env, HOLON_RUN_DIR: RUN_DIR, HOLON_TOKEN: "", HOLON_SERVER: "" } });
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
    // Its own name and port each run, so a reused server's earlier runs don't meet it.
    const name = `agent-${Date.now().toString(36)}`;
    const port = 9200 + Math.floor(Math.random() * 700);
    const text = (greeting: string) => `flow ${name}\nport ${port}\n\nroute GET /hi/:name (req) {\n  respond "${greeting}, {req.params.name}"\n}\n`;

    expect((await agent.tool("check", { text: text("Hi") })).text).toContain("GET /hi/:name");
    // The first deploy waits for a person.
    const proposed = await agent.tool("deploy", { text: text("Hi") });
    expect(proposed.failed).toBe(false);
    expect(proposed.text).toContain("Not deployed yet");
    expect(proposed.text).toContain("receive requests");

    // The person: the flows page lists it, saved by the agent's sign-in; they open it and deploy it, approving.
    await page.goto("/");
    await page.getByRole("button", { name: "flows", exact: true }).click();
    const row = page.getByText(name, { exact: true }).locator("xpath=..");
    await expect(row).toContainText("by local:");
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
