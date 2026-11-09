// A stand-in SMTP server for the end-to-end tests (the approval-mail sample's mail, server.mjs):
// plain on loopback, as a relay there may be, taking any sender and recipient and asking no login.
// Each mail it takes is a JSON file in `dir`, {from, to, data}, named by the order it came in
// (001.json, 002.json, …), so a test reads what the server sent as a mailbox would hold it.
//
// Given `token`, it also offers AUTH XOAUTH2 and takes that bearer token only, as Microsoft 365's
// and Google's do (the support-inbox sample's mail, logged in with OAuth): a mail sent logged in
// holds the user it logged in as, `login`. Another token is refused as theirs is: a challenge
// carrying the status, answered empty, then 535.

import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";

/** Listen on a free loopback port; resolves to it. */
export function smtpSink(dir, token) {
  let n = 0;
  const server = createServer((socket) => {
    let buffer = "";
    let mail = { from: "", to: [] };
    // The message's lines while DATA is open.
    let data = null;
    // The user an XOAUTH2 login was taken for, and whether one refused waits for its empty answer.
    let login = "";
    let refusing = false;
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
          writeFileSync(`${file}.part`, JSON.stringify({ ...mail, ...(login ? { login } : {}), data: `${data.join("\r\n")}\r\n` }));
          renameSync(`${file}.part`, file);
          mail = { from: "", to: [] };
          data = null;
          say("250 kept");
          continue;
        }
        if (refusing) {
          refusing = false;
          say("535 5.7.8 not that token");
          continue;
        }
        const verb = line.slice(0, 4).toUpperCase();
        if (verb === "EHLO" && token) say("250-e2e sink"), say("250 AUTH XOAUTH2");
        else if (verb === "EHLO" || verb === "HELO") say("250 e2e sink");
        else if (verb === "AUTH" && token && /^AUTH XOAUTH2 /i.test(line)) {
          // base64("user=U\x01auth=Bearer T\x01\x01")
          const [, user, bearer] = /^user=([^\x01]*)\x01auth=Bearer ([^\x01]*)\x01\x01$/.exec(Buffer.from(line.slice(13), "base64").toString("latin1")) ?? [];
          if (bearer === token) (login = user), say("235 logged in");
          else (refusing = true), say("334 eyJzdGF0dXMiOiI0MDAifQ==");
        }
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
