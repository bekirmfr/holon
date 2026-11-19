// @vitest-environment node
// A connection the server leaves unread (R97). On a busy machine the server's HTTP library
// (tiny_http's pool of connection threads) can queue a burst of new connections behind one
// worker that keeps its own for as long as its client does: a request written into the others
// gets no answer at all, until another connection closes. Here a stand-in server accepts the
// first connection and never reads it, and answers every other: the editor's read, given up at
// its time (limits.ts's ANSWER_WITHIN), drops that connection and is asked again on another,
// and is answered within its time and one try. With the real fetch (Node's), on real
// connections, not a stand-in for them; in Node, not the page's jsdom, whose AbortSignal Node's
// fetch doesn't take.

import { createServer, type Server, type Socket } from "node:net";
import { afterAll, beforeAll, expect, it, vi } from "vitest";

let server: Server;
let port = 0;
const connections: Socket[] = [];

beforeAll(async () => {
  server = createServer((socket) => {
    connections.push(socket);
    // The first accepted, never read: as the library's pool leaves it.
    if (connections.length === 1) return;
    socket.once("data", () => {
      const body = JSON.stringify({ setup: false, oidc: null });
      socket.end(`HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`);
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  port = (server.address() as { port: number }).port;
});

afterAll(async () => {
  connections.forEach((s) => s.destroy());
  await new Promise((r) => server.close(r));
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

it("a read on a connection the server never reads is given up, its connection dropped, and answered on another", async () => {
  // The page's window, for what server.ts tells it; the server, this one.
  vi.stubGlobal("window", new EventTarget());
  vi.stubEnv("VITE_HOLON_SERVER", `http://127.0.0.1:${port}`);
  vi.resetModules();
  const { ANSWER_WITHIN, READ_AGAIN_MS, getAuthState } = await import("./server");
  ANSWER_WITHIN.read = 500;
  READ_AGAIN_MS.splice(0, READ_AGAIN_MS.length, 100, 100);
  const from = performance.now();
  expect(await getAuthState()).toEqual({ setup: false, oidc: null });
  const took = performance.now() - from;
  // Answered on the second try: every try given up, it would be refused by 1.7 s, three tries.
  expect(connections, "asked again on a new connection").toHaveLength(2);
  // On the clock, loosely (a loaded runner): after its time (and the 100 ms before the second
  // try, more than a timer firing a millisecond early), and not hanging; about 0.6 s here.
  expect(took, "after its time").toBeGreaterThanOrEqual(500);
  expect(took, "not hanging").toBeLessThan(4_000);
  // Dropped by the editor, not left for the server to close: what is asked next doesn't wait on
  // it. Read at last, the first connection ends, closed from the editor's side (unread, Node
  // can't say so).
  const first = connections[0];
  const ended = new Promise<boolean>((r) => {
    first.once("end", () => r(true));
    setTimeout(() => r(false), 3_000);
  });
  first.resume();
  expect(await ended, "the first connection dropped by the editor").toBe(true);
}, 15_000);
