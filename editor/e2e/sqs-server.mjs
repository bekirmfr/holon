// A stand-in for Amazon SQS for the end-to-end tests (the payout-events sample's queues,
// server.mjs): its JSON protocol on plain HTTP on loopback, each request refused 403 unless it is
// signed (SigV4) with the key the tests give it, as SQS refuses one. Its queues are the names given,
// a FIFO one's ending .fifo. A message from outside is a file dropped in its queue's folder in
// `dir` (`dir/payout-events/…`): its text the message's body, put in the order of their names (the
// folders are looked at every 200ms). What the host sends is a line of `sent.log` in `dir`, and
// what it deletes a line of `deleted.log`, each JSON, where the tests read them.
//
// A message received is out of sight for its visibility, then handed out again unless it was
// deleted; a FIFO queue hands out none of a group while one of it is out of sight, and takes a
// message whose deduplication id it has seen as the same message.

import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { createHash, createHmac } from "node:crypto";
import { createServer } from "node:http";
import { join } from "node:path";

const sha = (data) => createHash("sha256").update(data).digest("hex");
const mac = (key, data) => createHmac("sha256", key).update(data).digest();

/** Whether `req`'s Authorization is the one `secret` makes over what it sent, for `keyId`. */
function signed(req, body, keyId, secret) {
  const m = /^AWS4-HMAC-SHA256 Credential=([^/]+)\/(\d{8})\/([^/]+)\/([^/]+)\/aws4_request, SignedHeaders=([^,]+), Signature=([0-9a-f]+)$/.exec(req.headers.authorization ?? "");
  if (!m || m[1] !== keyId || m[4] !== "sqs") return false;
  const [, , day, region, service, names, signature] = m;
  const lines = names
    .split(";")
    .map((n) => `${n}:${String(req.headers[n] ?? "").trim()}\n`)
    .join("");
  const request = `${req.method}\n/\n\n${lines}\n${names}\n${sha(body)}`;
  const toSign = `AWS4-HMAC-SHA256\n${req.headers["x-amz-date"] ?? ""}\n${day}/${region}/${service}/aws4_request\n${sha(request)}`;
  const key = [region, service, "aws4_request"].reduce((k, part) => mac(k, part), mac(`AWS4${secret}`, day));
  return createHmac("sha256", key).update(toSign).digest("hex") === signature;
}

/** Listen on a free loopback port, answering for the queues `names` with the key `keyId` and
 *  `secret`; resolves to the port. */
export function sqsServer(dir, names, keyId, secret) {
  const queues = new Map(names.map((n) => [n, []]));
  const seen = new Set();
  let made = 0;
  const put = (queue, body, group, dedup) => {
    const message = { id: `e2e-${String(++made).padStart(6, "0")}`, body, group, dedup, sent: Date.now(), count: 0, receipt: null, visibleAt: 0 };
    queues.get(queue).push(message);
    return message;
  };
  const look = () => {
    for (const name of names) {
      const folder = join(dir, name);
      if (!existsSync(folder)) continue;
      for (const file of readdirSync(folder).sort().filter((f) => !seen.has(`${name}/${f}`))) {
        seen.add(`${name}/${file}`);
        put(name, readFileSync(join(folder, file), "utf8").trim(), name.endsWith(".fifo") ? "e2e" : undefined);
      }
    }
  };
  for (const name of names) mkdirSync(join(dir, name), { recursive: true });
  setInterval(look, 200).unref();
  const log = (file, line) => existsSync(dir) && appendFileSync(join(dir, file), `${JSON.stringify(line)}\n`);

  const answer = (res, status, json) => res.writeHead(status, { "Content-Type": "application/x-amz-json-1.0" }).end(JSON.stringify(json));
  const refuse = (res, status, kind, message) => answer(res, status, { __type: `com.amazonaws.sqs#${kind}`, message });
  const server = createServer((req, res) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", async () => {
      const body = Buffer.concat(chunks);
      if (req.method !== "POST" || !signed(req, body, keyId, secret)) {
        refuse(res, 403, "SignatureDoesNotMatch", "The request signature we calculated does not match the signature you provided.");
        return;
      }
      const action = String(req.headers["x-amz-target"] ?? "").replace(/^AmazonSQS\./, "");
      let json = {};
      try {
        json = JSON.parse(body.toString("utf8"));
      } catch {
        refuse(res, 400, "InvalidParameterValue", "The request isn't JSON.");
        return;
      }
      const name = String(json.QueueUrl ?? "").split("/").pop();
      const queue = queues.get(name);
      if (!queue) {
        refuse(res, 400, "QueueDoesNotExist", "The specified queue does not exist.");
        return;
      }
      const fifo = name.endsWith(".fifo");
      if (action === "GetQueueAttributes") {
        answer(res, 200, { Attributes: { VisibilityTimeout: "30", MaximumMessageSize: "262144", ...(fifo ? { FifoQueue: "true" } : {}) } });
      } else if (action === "SendMessage") {
        const group = json.MessageGroupId;
        if (fifo && !group) {
          refuse(res, 400, "MissingParameter", "The request must contain the parameter MessageGroupId.");
          return;
        }
        const dedup = json.MessageDeduplicationId;
        const same = fifo && dedup ? queue.find((m) => m.dedup === dedup) : undefined;
        const message = same ?? put(name, String(json.MessageBody ?? ""), group, dedup);
        if (!same) log("sent.log", { queue: name, id: message.id, body: message.body, group: group ?? null, dedup: dedup ?? null });
        answer(res, 200, { MessageId: message.id });
      } else if (action === "ReceiveMessage") {
        // A long poll waits for a message, as long as it asks (20s at most).
        const until = Date.now() + Math.min(Number(json.WaitTimeSeconds ?? 0), 20) * 1000;
        for (;;) {
          look();
          const now = Date.now();
          const held = new Set(queue.filter((m) => fifo && m.visibleAt > now).map((m) => m.group));
          const given = [];
          for (const m of queue) {
            if (given.length >= Number(json.MaxNumberOfMessages ?? 1)) break;
            if (m.visibleAt > now || held.has(m.group)) continue;
            m.count += 1;
            m.visibleAt = now + Number(json.VisibilityTimeout ?? 30) * 1000;
            m.receipt = `${m.id}-r${m.count}`;
            given.push({
              MessageId: m.id,
              ReceiptHandle: m.receipt,
              Body: m.body,
              Attributes: { SentTimestamp: String(m.sent), ApproximateReceiveCount: String(m.count), ...(m.group ? { MessageGroupId: m.group } : {}) },
            });
          }
          if (given.length > 0 || Date.now() >= until || req.destroyed) {
            answer(res, 200, given.length > 0 ? { Messages: given } : {});
            return;
          }
          await new Promise((r) => setTimeout(r, 100));
        }
      } else if (action === "DeleteMessage" || action === "ChangeMessageVisibility") {
        const at = queue.findIndex((m) => m.receipt === json.ReceiptHandle);
        if (at < 0) {
          refuse(res, 400, "ReceiptHandleIsInvalid", "The input receipt handle is invalid.");
          return;
        }
        if (action === "DeleteMessage") {
          const [m] = queue.splice(at, 1);
          log("deleted.log", { queue: name, id: m.id, body: m.body });
        } else {
          queue[at].visibleAt = Date.now() + Number(json.VisibilityTimeout ?? 0) * 1000;
        }
        answer(res, 200, {});
      } else {
        refuse(res, 400, "InvalidAction", "The action or operation requested is invalid.");
      }
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}
