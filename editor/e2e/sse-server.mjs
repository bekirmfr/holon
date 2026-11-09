// A stand-in feed of server-sent events for the end-to-end tests (the price-feed sample's feed,
// server.mjs): plain HTTP on loopback, at /events. Its events are files dropped in `dir`: each
// .json file is one event, its name (without .json) the event's id and its text the event's data,
// sent to every connection open once it is seen (the folder is looked at every 200ms, new files
// in the order of their names). It keeps what it sent, so a connection that opens with
// `Last-Event-ID` gets what came after that event first; one without it, what comes from now.
//
// A file named `cut` ends every connection open (it is removed), as a feed that drops its
// connections does; each connection's Last-Event-ID (or "-" without one) is a line of
// `connections.log` in `dir`, where the tests read where the host opened it again from.

import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";

/** Listen on a free loopback port, sending the events dropped in `dir`; resolves to the port. */
export function sseServer(dir) {
  mkdirSync(dir, { recursive: true });
  // Each event sent: its id and its data, in the order they were seen.
  const sent = [];
  const seen = new Set();
  const open = new Set();
  const look = () => {
    if (!existsSync(dir)) return;
    const names = readdirSync(dir).sort();
    if (names.includes("cut")) {
      rmSync(join(dir, "cut"), { force: true });
      for (const res of open) res.destroy();
      open.clear();
    }
    for (const name of names.filter((n) => n.endsWith(".json") && !seen.has(n))) {
      seen.add(name);
      const event = { id: name.slice(0, -".json".length), data: readFileSync(join(dir, name), "utf8").trim() };
      sent.push(event);
      for (const res of open) res.write(block(event));
    }
  };
  setInterval(look, 200).unref();
  const block = (e) => `id: ${e.id}\n${e.data.split("\n").map((l) => `data: ${l}`).join("\n")}\n\n`;
  const server = createServer((req, res) => {
    if (req.method !== "GET" || req.url.split("?")[0] !== "/events") {
      res.writeHead(404).end();
      return;
    }
    look();
    const last = req.headers["last-event-id"];
    if (existsSync(dir)) appendFileSync(join(dir, "connections.log"), `${last ?? "-"}\n`);
    const after = last === undefined ? -1 : sent.findIndex((e) => e.id === last);
    res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" });
    res.write(": open\n\n");
    for (const e of after < 0 ? [] : sent.slice(after + 1)) res.write(block(e));
    open.add(res);
    // Kept alive, as feeds are.
    const alive = setInterval(() => res.write(": alive\n\n"), 15_000);
    res.on("close", () => {
      clearInterval(alive);
      open.delete(res);
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}
