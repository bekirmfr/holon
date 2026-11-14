// What a Resources table's row says of each kind of binding (resourceRows.ts; web-console-L4.md
// §9.1): where it is, to whoever may bind resources only, and never "undefined" when the server
// left it out (L4 Q4); the rest in a few words.

import { describe, expect, it } from "vitest";
import { databaseLine, feedLine, folderLine, lineLabel, mailLine, queueLine } from "./resourceRows";
import type { DatabaseEntry, FeedEntry, FolderEntry, MailEntry, QueueEntry } from "./server";

describe("a resource's row", () => {
  it("says a folder's path to whoever may bind, and nothing where the server left it out", () => {
    const folder: FolderEntry = { name: "inbox", path: "/srv/inbox", access: "read", keep: "bytes for 7d", cap: 10 * 2 ** 20, shared: true, lock: "run", deployment: "reports" };
    expect(folderLine(folder, true)).toEqual({
      kind: "folder",
      name: "inbox",
      deployment: "reports",
      where: "/srv/inbox",
      notes: ["read only", "reads kept 7d, then by hash", "capped at 10 MiB", "other servers bind it too", "held for whole runs"],
    });
    // A viewer's list has no path (the server's), and a viewer isn't shown one the editor has.
    const { path: _, ...hidden } = folder;
    expect(folderLine(hidden, true).where).toBeNull();
    expect(folderLine(folder, false).where).toBeNull();
    expect(JSON.stringify(folderLine(hidden, true))).not.toContain("undefined");
    expect(lineLabel(folderLine(folder, true))).toBe("folder inbox for reports");
  });

  it("says where a database is: its file, or its connection's address and source; nothing to a viewer", () => {
    const sqlite: DatabaseEntry = { name: "shop", engine: "sqlite", path: "/srv/shop.db", access: "read-write", deployment: null };
    expect(databaseLine(sqlite, true)).toEqual({ kind: "database", name: "shop", deployment: null, where: "/srv/shop.db", notes: ["sqlite", "read and write"] });
    const pg: DatabaseEntry = { name: "ledger", engine: "postgres", connection: "env:HOLON_SECRET_LEDGER_URL", address: "postgres://db.example/ledger", access: "read", deployment: null };
    expect(databaseLine(pg, true).where).toBe("postgres://db.example/ledger (from env:HOLON_SECRET_LEDGER_URL)");
    expect(databaseLine({ ...pg, address: undefined }, true).where).toBe("from env:HOLON_SECRET_LEDGER_URL");
    expect(databaseLine({ name: "ledger", engine: "postgres", access: "read", deployment: null }, true).where).toBeNull();
    expect(databaseLine(pg, false).where).toBeNull();
    expect(lineLabel(databaseLine(pg, true))).toBe("database ledger");
  });

  it("says a mail's server, its mailbox and how it logs in", () => {
    const mail: MailEntry = {
      name: "ops",
      deployment: null,
      smtp: "env:HOLON_SECRET_OPS_SMTP",
      address: "smtp://mail.example:587",
      from: "Treasury <treasury@acme.com>",
      to: ["*@acme.com"],
      mailbox: { imap: "env:HOLON_SECRET_OPS_IMAP", address: "imap://mail.example", folder: "INBOX", done: "Processed", every: 30 },
      oauth: { source: "env:HOLON_SECRET_OPS_OAUTH", address: "https://login.example/token" },
    };
    const line = mailLine(mail);
    expect(line.where).toBe(
      "smtp://mail.example:587 (from env:HOLON_SECRET_OPS_SMTP) · as Treasury <treasury@acme.com> · to *@acme.com · receives in imap://mail.example INBOX (from env:HOLON_SECRET_OPS_IMAP), moved to Processed · no verdict believed",
    );
    expect(line.notes).toEqual(["logs in with OAuth (https://login.example/token from env:HOLON_SECRET_OPS_OAUTH)"]);
    expect(mailLine({ ...mail, smtp: "", mailbox: null, oauth: null }).where).toBeNull();
  });

  it("says a feed's and a queue's address, and what they keep", () => {
    const feed: FeedEntry = { name: "prices", deployment: null, url: "env:HOLON_SECRET_PRICES_URL", address: "http://127.0.0.1:9/events", headers: {}, buffer: 1000, idle: 90, on_full: "wait" };
    expect(feedLine(feed).where).toBe("http://127.0.0.1:9/events (from env:HOLON_SECRET_PRICES_URL) · 1000 may wait · quiet for 90s at most");
    expect(feedLine({ ...feed, on_full: "latest", key: "product_id", database: "ledger" }).where).toBe(
      "listens on http://127.0.0.1:9/events (the database ledger) · 1000 may wait, one a product_id, the newest · quiet for 90s at most",
    );
    const queue: QueueEntry = { name: "retries", deployment: "payouts", url: "env:HOLON_SECRET_RETRIES_URL", address: "http://sqs.example/1/retries.fifo", fifo: true, largest: 262144, visibility: 300, keep_failed: true };
    expect(queueLine(queue).where).toBe(
      "http://sqs.example/1/retries.fifo (from env:HOLON_SECRET_RETRIES_URL) · FIFO · 256 KiB at most · out of sight for 300s · a failed run's message is left for the redrive",
    );
    expect(lineLabel(queueLine(queue))).toBe("queue retries for payouts");
  });
});
