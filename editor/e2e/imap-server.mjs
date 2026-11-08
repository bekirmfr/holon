// A stand-in IMAP server for the end-to-end tests (the remittance-mail sample's mailbox, server.mjs):
// plain on loopback, taking any login (LOGIN; it offers no AUTH=PLAIN), its folders the folders in
// `dir`, each message an .eml file there. A test drops a message in as a file (INBOX/r1.eml), and
// one the server moves is a file moved to the folder it names (Processed/r1.eml), so a test reads
// the mailbox as the folders hold it.
//
// UIDs: each file a folder holds is given the next one when it is first listed; UIDVALIDITY is
// fixed. It speaks what Holon's client says (crates/holon/src/imap.rs): CAPABILITY, LOGIN, SELECT,
// UID SEARCH ALL, UID FETCH (RFC822.SIZE or BODY.PEEK[]), UID MOVE, CREATE, LIST, NOOP, LOGOUT.

import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";

/** Listen on a free loopback port, serving the folders in `dir` (INBOX made if it isn't there);
 *  resolves to the port. */
export function imapServer(dir) {
  mkdirSync(join(dir, "INBOX"), { recursive: true });
  // Each folder's files by UID, and the next UID.
  const folders = new Map();
  const uidsOf = (folder) => {
    if (!folders.has(folder)) folders.set(folder, { byName: new Map(), next: 1 });
    const f = folders.get(folder);
    const now = readdirSync(join(dir, folder)).filter((n) => n.endsWith(".eml")).sort();
    for (const name of now) if (!f.byName.has(name)) f.byName.set(name, f.next++);
    for (const name of [...f.byName.keys()]) if (!now.includes(name)) f.byName.delete(name);
    return f.byName;
  };
  const named = (folder, uid) => [...uidsOf(folder)].find(([, u]) => u === uid)?.[0];
  const isFolder = (name) => /^[\x20-\x7e]+$/.test(name) && !/["\\&%*]|\.\./.test(name) && existsSync(join(dir, name));
  // A command's words, a quoted string one word.
  const words = (line) => [...line.matchAll(/"((?:[^"\\]|\\.)*)"|(\S+)/g)].map((m) => (m[1] !== undefined ? m[1].replace(/\\(.)/g, "$1") : m[2]));

  const server = createServer((socket) => {
    let buffer = "";
    let selected = null;
    const say = (text) => socket.write(text);
    say("* OK e2e mailbox ready\r\n");
    socket.on("data", (chunk) => {
      buffer += chunk.toString("latin1");
      for (let at = buffer.indexOf("\r\n"); at >= 0; at = buffer.indexOf("\r\n")) {
        const line = buffer.slice(0, at);
        buffer = buffer.slice(at + 2);
        const [tag, verb = "", ...args] = words(line);
        const ok = `${tag} OK done\r\n`;
        const no = `${tag} NO not that\r\n`;
        switch (verb.toUpperCase()) {
          case "CAPABILITY":
            say(`* CAPABILITY IMAP4rev1 MOVE UIDPLUS\r\n${ok}`);
            break;
          case "LOGIN":
          case "NOOP":
            say(ok);
            break;
          case "SELECT": {
            if (!isFolder(args[0] ?? "")) {
              say(no);
              break;
            }
            selected = args[0];
            say(`* ${uidsOf(selected).size} EXISTS\r\n* OK [UIDVALIDITY 1] UIDs valid\r\n${tag} OK [READ-WRITE] selected\r\n`);
            break;
          }
          case "CREATE":
            if (isFolder(args[0] ?? "")) say(`${tag} NO [ALREADYEXISTS] it is there\r\n`);
            else if (/^[\x20-\x7e]+$/.test(args[0] ?? "") && !/["\\&%*/]|\.\./.test(args[0])) mkdirSync(join(dir, args[0]), { recursive: true }), say(ok);
            else say(no);
            break;
          case "LIST":
            say(`${isFolder(args[1] ?? "") ? `* LIST () "/" "${args[1]}"\r\n` : ""}${ok}`);
            break;
          case "UID": {
            const sub = (args[0] ?? "").toUpperCase();
            if (!selected) {
              say(`${tag} BAD select a folder first\r\n`);
              break;
            }
            if (sub === "SEARCH") {
              say(`* SEARCH ${[...uidsOf(selected).values()].join(" ")}\r\n${ok}`);
              break;
            }
            const uid = Number(args[1]);
            const name = named(selected, uid);
            if (sub === "FETCH") {
              if (!name) {
                say(ok);
                break;
              }
              const bytes = readFileSync(join(dir, selected, name));
              if (line.includes("RFC822.SIZE")) say(`* 1 FETCH (UID ${uid} RFC822.SIZE ${bytes.length})\r\n${ok}`);
              else socket.write(Buffer.concat([Buffer.from(`* 1 FETCH (UID ${uid} BODY[] {${bytes.length}}\r\n`, "latin1"), bytes, Buffer.from(` FLAGS ())\r\n${ok}`, "latin1")]));
              break;
            }
            if (sub === "MOVE") {
              const to = args[2] ?? "";
              if (!name || !isFolder(to)) {
                say(no);
                break;
              }
              renameSync(join(dir, selected, name), join(dir, to, name));
              say(ok);
              break;
            }
            say(`${tag} BAD not here\r\n`);
            break;
          }
          case "LOGOUT":
            say(`* BYE bye\r\n${ok}`);
            socket.end();
            break;
          default:
            say(`${tag} BAD not here\r\n`);
        }
      }
    });
    socket.on("error", () => {});
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}
