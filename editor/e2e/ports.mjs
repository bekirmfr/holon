// The end-to-end tests' ports, by slot, so runs can go side by side (two worktrees, two sessions,
// a developer and an agent). Slot n moves every port by n × 100: the main test server (7520), the
// one left as installed (7521), the stand-in identity provider (7530), the two editors (5273,
// 5274), and the ports samples deploy on (their own plus 1000). The run files the command line
// reads go in a folder of the slot's own, so `holon export --server` sees only its run's servers.
//
// HOLON_E2E_SLOT names the slot. Unset, the runner takes the first slot whose ports are free and
// that no other run has claimed (a lock file beside the run files, gone when the run ends, or found
// stale when its process is), then sets HOLON_E2E_SLOT so its servers and workers read the same.
// Slots 0 to 9: past 9 a slot's sample ports would meet slot 0's.

import { execFileSync } from "node:child_process";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SLOTS = 10;
const DEFAULTS = { server: 7520, fresh: 7521, oidc: 7530, editor: 5273, accountsEditor: 5274 };
const portsOf = (slot) => Object.fromEntries(Object.entries(DEFAULTS).map(([k, p]) => [k, p + slot * 100]));
const lockOf = (slot) => join(tmpdir(), `holon-e2e-slot-${slot}.lock`);

/** Whether every port is free, on IPv4 and IPv6 loopback both (vite listens on "localhost"). */
function free(ports) {
  const probe = `
    const net = require("node:net");
    const one = (port, host) => new Promise((ok) => {
      const s = net.createServer();
      s.once("error", (e) => ok(e.code === "EADDRNOTAVAIL" || e.code === "EAFNOSUPPORT"));
      s.listen(port, host, () => s.close(() => ok(true)));
    });
    Promise.all(${JSON.stringify(ports)}.flatMap((p) => [one(p, "127.0.0.1"), one(p, "::1")]))
      .then((all) => process.stdout.write(all.every(Boolean) ? "free" : "taken"));`;
  return execFileSync(process.execPath, ["-e", probe], { encoding: "utf8" }) === "free";
}

/** Claim the slot's lock, or say another live run holds it. */
function claim(slot) {
  const lock = lockOf(slot);
  try {
    writeFileSync(lock, String(process.pid), { flag: "wx" });
    return true;
  } catch {
    const pid = Number(readFileSync(lock, "utf8"));
    try {
      process.kill(pid, 0);
      return false;
    } catch {
      // Its run is gone: the lock is stale.
      rmSync(lock, { force: true });
      return claim(slot);
    }
  }
}

function pick() {
  for (let slot = 0; slot < SLOTS; slot++) {
    if (!free(Object.values(portsOf(slot)))) continue;
    if (!claim(slot)) continue;
    process.on("exit", () => rmSync(lockOf(slot), { force: true }));
    return slot;
  }
  throw new Error(`e2e: no free slot among 0 to ${SLOTS - 1}; set HOLON_E2E_SLOT to choose one`);
}

if (process.env.HOLON_E2E_SLOT === undefined) process.env.HOLON_E2E_SLOT = String(pick());

/** This run's slot. */
export const SLOT = Number(process.env.HOLON_E2E_SLOT);
if (!Number.isInteger(SLOT) || SLOT < 0 || SLOT >= SLOTS) throw new Error(`e2e: HOLON_E2E_SLOT is ${process.env.HOLON_E2E_SLOT}; it takes 0 to ${SLOTS - 1}`);

/** This run's ports: `server`, `fresh`, `oidc`, `editor`, `accountsEditor`. */
export const PORTS = portsOf(SLOT);
/** What a sample's port is moved up by to deploy here: off its own port, and off other slots'. */
export const SAMPLE_SHIFT = 1000 + SLOT * 100;
/** Where this run's servers write their run files (local.rs), never the developer's ~/.holon/run. */
export const RUN_DIR = join(tmpdir(), `holon-e2e-run-${SLOT}`);
