// A stand-in SMTP server for the end-to-end tests (the approval-mail sample's mail, server.mjs):
// plain on loopback, as a relay there may be, taking any sender and recipient and asking no login.
// Each mail it takes is a JSON file in `dir`, {from, to, data}, named by the order it came in
// (001.json, 002.json, …), so a test reads what the server sent as a mailbox would hold it.

import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";

/** Listen on a free loopback port; resolves to it. */
export function smtpSink(dir) {
  let n = 0;
  const server = createServer((socket) => {
    let buffer = "";
    let mail = { from: "", to: [] };
    // The message's lines while DATA is open.
    let data = null;
    const say = (line) => socket.write(`${line}\r\n`);
    const address = (line) => /<([^>]*)>/.exec(line)?.[1] ?? "";
    say("220 e2e sink");
    socket.on("data", (chunk) => {
      // What Holon writes is ASCII (quoted-printable, base64, RFC 2047): byte for character.
      buffer += chunk.toString("latin1");
      for (let at = buffer.indexOf("\r\n"); at >= 0; at = buffer.indexOf("\r\n")) {
        const line = buffer.slice(0, at);
        buffer = buffer.slice(at + 2);
        if (data) {
          if (line !== ".") {
            data.push(line.startsWith(".") ? line.slice(1) : line);
            continue;
          }
          n += 1;
          mkdirSync(dir, { recursive: true });
          const file = join(dir, `${String(n).padStart(3, "0")}.json`);
          // Whole or not at all, for a test reading the folder as it fills.
          writeFileSync(`${file}.part`, JSON.stringify({ ...mail, data: `${data.join("\r\n")}\r\n` }));
          renameSync(`${file}.part`, file);
          mail = { from: "", to: [] };
          data = null;
          say("250 kept");
          continue;
        }
        const verb = line.slice(0, 4).toUpperCase();
        if (verb === "EHLO" || verb === "HELO") say("250 e2e sink");
        else if (verb === "MAIL") (mail.from = address(line)), say("250 ok");
        else if (verb === "RCPT") mail.to.push(address(line)), say("250 ok");
        else if (verb === "DATA") (data = []), say("354 go on");
        else if (verb === "RSET") (mail = { from: "", to: [] }), say("250 ok");
        else if (verb === "QUIT") say("221 bye"), socket.end();
        else say("502 not here");
      }
    });
    socket.on("error", () => {});
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}
