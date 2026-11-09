// A stand-in WebSocket feed for the end-to-end tests (the trading-bot sample's ticker, the
// bluesky-mentions sample's posts and the trade-tape sample's trades, server.mjs):
// plain ws:// on loopback, at /ws, written by hand (the handshake and its frames), since Node has
// no WebSocket server of its own. Its messages are files dropped in `dir`: each .json file is one
// text message, sent to every connection open once it is seen (the folder is looked at every
// 200ms, new files in the order of their names); a connection gets what comes once it is open.
//
// What a connection sends it, its subscription, is a line of `heard.log` in `dir`, where the tests
// read it, and what each asked for when it opened (its path and query) a line of `asked.log`; a
// ping is answered with its pong, and a file named `cut` ends every connection open (it is
// removed), as a feed that drops its connections does. A connection that asks from a `cursor=N`
// is sent again what was sent from the message whose `seq` (or `payload.seq`) is N, as Jetstream
// does.

import { createHash } from "node:crypto";
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";

/** What the handshake's accept is made with (RFC 6455). */
const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/** A frame from the server: whole, unmasked, its opcode and its payload. */
function frame(opcode, payload) {
  const length = payload.length;
  let head;
  if (length < 126) {
    head = Buffer.from([0x80 | opcode, length]);
  } else if (length < 65536) {
    head = Buffer.from([0x80 | opcode, 126, length >> 8, length & 255]);
  } else {
    head = Buffer.alloc(10);
    head[0] = 0x80 | opcode;
    head[1] = 127;
    head.writeBigUInt64BE(BigInt(length), 2);
  }
  return Buffer.concat([head, payload]);
}

/** Listen on a free loopback port, sending the messages dropped in `dir`; resolves to the port. */
export function wsServer(dir) {
  mkdirSync(dir, { recursive: true });
  const seen = new Set();
  const open = new Set();
  // Each message sent, in order, for a connection that asks from a cursor.
  const sent = [];
  const seq = (text) => {
    try {
      const v = JSON.parse(text);
      return v.seq ?? v.payload?.seq;
    } catch {
      return undefined;
    }
  };
  const look = () => {
    if (!existsSync(dir)) return;
    const names = readdirSync(dir).sort();
    if (names.includes("cut")) {
      rmSync(join(dir, "cut"), { force: true });
      for (const socket of open) socket.destroy();
      open.clear();
    }
    for (const name of names.filter((n) => n.endsWith(".json") && !seen.has(n))) {
      seen.add(name);
      const text = readFileSync(join(dir, name), "utf8").trim();
      sent.push(text);
      const message = frame(1, Buffer.from(text));
      for (const socket of open) socket.write(message);
    }
  };
  setInterval(look, 200).unref();
  // Kept alive, as feeds are: a ping now and then, which the host answers.
  setInterval(() => {
    for (const socket of open) socket.write(frame(9, Buffer.from("alive")));
  }, 15_000).unref();
  const server = createServer((req, res) => res.writeHead(426).end());
  server.on("upgrade", (req, socket) => {
    const key = req.headers["sec-websocket-key"];
    if (req.url.split("?")[0] !== "/ws" || !key || (req.headers.upgrade ?? "").toLowerCase() !== "websocket") {
      socket.end("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
      return;
    }
    const accept = createHash("sha1").update(key + GUID).digest("base64");
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    if (existsSync(dir)) appendFileSync(join(dir, "asked.log"), `${req.url}\n`);
    // What was dropped before it opened was sent before: it gets what comes from now, and again
    // what came from its cursor.
    look();
    const cursor = Number(new URL(req.url, "http://feed").searchParams.get("cursor") ?? NaN);
    if (!Number.isNaN(cursor)) for (const text of sent.filter((t) => seq(t) >= cursor)) socket.write(frame(1, Buffer.from(text)));
    open.add(socket);
    let pending = Buffer.alloc(0);
    socket.on("data", (chunk) => {
      pending = Buffer.concat([pending, chunk]);
      for (;;) {
        if (pending.length < 2) return;
        const opcode = pending[0] & 15;
        let length = pending[1] & 127;
        let at = 2;
        if (length === 126) {
          if (pending.length < 4) return;
          length = pending.readUInt16BE(2);
          at = 4;
        } else if (length === 127) {
          if (pending.length < 10) return;
          length = Number(pending.readBigUInt64BE(2));
          at = 10;
        }
        // A client's frames are masked.
        const mask = pending[1] & 128 ? pending.subarray(at, at + 4) : null;
        if (mask) at += 4;
        if (pending.length < at + length) return;
        const payload = Buffer.from(pending.subarray(at, at + length));
        if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
        pending = pending.subarray(at + length);
        if (opcode === 1 && existsSync(dir)) appendFileSync(join(dir, "heard.log"), `${payload.toString("utf8")}\n`);
        else if (opcode === 9) socket.write(frame(10, payload));
        else if (opcode === 8) {
          open.delete(socket);
          socket.end(frame(8, payload.subarray(0, 2)));
          return;
        }
      }
    });
    socket.on("close", () => open.delete(socket));
    socket.on("error", () => open.delete(socket));
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}
