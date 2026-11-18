// A tab reopened after a reload opens as what the server holds under its name when it reopens
// (reopen.ts): the flow saved so, else the deployment, else nothing. On the e2e's slow runner the
// page read whether it was deployed from a list it hadn't loaded yet, and told the person their
// deployed flow didn't exist. Each case: what the server holds, what the tab reopens as, and that
// the server was asked then.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { reopening } from "./reopen";
import { setSession } from "./server";

// A tab reopens signed in (with no one, nothing is asked: server.ts's `api`).
beforeEach(() => setSession({ token: "t", name: "cy", kind: "person", role: "editor" }));

/** The server: `saved` the flows saved, `deployed` the slugs deployed, each read when asked. */
function server(held: { saved: string[]; deployed: string[] }) {
  const asked: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string) => {
      const path = new URL(url, "http://127.0.0.1:7420").pathname;
      asked.push(path);
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
      if (path === "/deployments") return json({ deployments: held.deployed.map((slug) => ({ slug, port: null })) });
      const name = decodeURIComponent(path.replace(/^\/flows\//, ""));
      return held.saved.includes(name) ? json({ name, document: { nodes: [], edges: [] } }) : json({ error: `no flow "${name}"` }, 404);
    }),
  );
  return asked;
}

afterEach(() => {
  vi.unstubAllGlobals();
  setSession(null);
});

describe("a tab reopened after a reload", () => {
  it("opens the flow saved under its name, asking nothing more", async () => {
    const asked = server({ saved: ["tally"], deployed: ["tally"] });
    const found = await reopening("tally");
    expect(found.kind).toBe("saved");
    expect(found.kind === "saved" && found.flow.name).toBe("tally");
    expect(asked).toEqual(["/flows/tally"]);
  });

  it("never saved but deployed, opens the deployment, as the server lists it then", async () => {
    const asked = server({ saved: [], deployed: ["notes-api", "hello-api"] });
    expect(await reopening("hello-api")).toEqual({ kind: "deployed" });
    expect(asked).toEqual(["/flows/hello-api", "/deployments"]);
  });

  it("neither saved nor deployed, has nothing to reopen", async () => {
    server({ saved: [], deployed: ["notes-api"] });
    expect(await reopening("hello-api")).toEqual({ kind: "none" });
  });

  it("follows the server from one reopen to the next, keeping no list of its own", async () => {
    const held = { saved: [] as string[], deployed: [] as string[] };
    server(held);
    expect(await reopening("hello-api")).toEqual({ kind: "none" });
    held.deployed.push("hello-api");
    expect(await reopening("hello-api")).toEqual({ kind: "deployed" });
    held.saved.push("hello-api");
    expect((await reopening("hello-api")).kind).toBe("saved");
  });

  it("a server that can't list its deployments leaves nothing to reopen, not a failure", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("down", { status: 503 })),
    );
    expect(await reopening("hello-api")).toEqual({ kind: "none" });
  });
});
