// Test vectors for holon-core's pgp.rs, made by GnuPG: keys of each kind (one expired, one revoked,
// a stranger's), detached signatures over a document, messages GnuPG encrypted, and the two banks'
// secret keys, unprotected, for the test that decrypts. Written to pgp-vectors.json:
//   node crates/holon-core/src/testdata/pgp-vectors.mjs
// Throwaway keys, made for these tests: they protect nothing.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const home = mkdtempSync(join(tmpdir(), "holon-pgp-"));
// Git for Windows's gpg (MSYS) finds its agent only through a POSIX path: /c/Users/…
const homedir = process.platform === "win32" ? home.replace(/^([A-Za-z]):/, (_, d) => `/${d.toLowerCase()}`).replaceAll("\\", "/") : home;
const gpg = (args, input) =>
  execFileSync("gpg", ["--homedir", homedir, "--batch", "--yes", "--pinentry-mode", "loopback", "--passphrase", "", "--trust-model", "always", ...args], {
    input,
    maxBuffer: 1 << 24,
    stdio: ["pipe", "pipe", "ignore"],
  });
const at = (time) => [`--faked-system-time`, `${time}!`];
const [KEYS, SIGNED, OLD, LATER] = ["20260101T000000", "20260601T000000", "20250101T000000", "20270101T000000"];

// Fingerprints in the order gpg lists them: the primary, then each subkey.
const fingerprints = (uid) =>
  gpg(["--with-colons", "--list-keys", uid])
    .toString()
    .split("\n")
    .filter((l) => l.startsWith("fpr:"))
    .map((l) => l.split(":")[9]);
const make = (uid, algo, usage, expire, time, subkeys = []) => {
  gpg([...at(time), "--quick-gen-key", uid, algo, usage, expire]);
  const [primary] = fingerprints(uid);
  for (const [a, u, e] of subkeys) gpg([...at(time), "--quick-add-key", primary, a, u, e]);
  return fingerprints(uid);
};

const rsa = make("Bank RSA <rsa@bank.example>", "rsa3072", "sign", "never", KEYS, [["rsa3072", "encr", "never"]]);
const c25519 = make("Bank 25519 <c25519@bank.example>", "ed25519", "sign", "never", KEYS, [["cv25519", "encr", "never"], ["ed25519", "sign", "never"]]);
const old = make("Old Bank <old@bank.example>", "ed25519", "sign", "30d", OLD, [["cv25519", "encr", "30d"]]);
const gone = make("Gone Bank <gone@bank.example>", "ed25519", "sign", "never", KEYS, [["cv25519", "encr", "never"]]);
const other = make("Someone Else <other@example.org>", "ed25519", "sign", "never", KEYS);

const data = "<Document>pain.002 for batch B-1</Document>\nstatus ACCP\n";
const sign = (key, time, extra = []) => gpg([...at(time), "--detach-sign", "-u", `${key}!`, ...extra], data);
const b64 = (b) => Buffer.from(b).toString("base64");
const text = (b) => Buffer.from(b).toString("utf8");
const signatures = {
  rsa_sha256: b64(sign(rsa[0], SIGNED, ["--digest-algo", "SHA256"])),
  rsa_sha512_armored: text(sign(rsa[0], SIGNED, ["--digest-algo", "SHA512", "--armor"])),
  rsa_sha1: b64(sign(rsa[0], SIGNED, ["--digest-algo", "SHA1"])),
  ed_primary: b64(sign(c25519[0], SIGNED)),
  ed_subkey_armored: text(sign(c25519[2], SIGNED, ["--armor"])),
  ed_text: b64(sign(c25519[0], SIGNED, ["--textmode"])),
  old_while_valid: b64(sign(old[0], "20250110T000000")),
  gone_before_revoked: b64(sign(gone[0], SIGNED)),
  other: b64(sign(other[0], SIGNED)),
  future: b64(sign(c25519[0], LATER)),
};
signatures.other_then_rsa = b64(Buffer.concat([Buffer.from(signatures.other, "base64"), Buffer.from(signatures.rsa_sha256, "base64")]));

// The revoked key: its revocation certificate, as gpg made it with the key, imported.
const revocation = readFileSync(join(home, "openpgp-revocs.d", `${gone[0]}.rev`), "utf8").replace(":-----BEGIN", "-----BEGIN");
gpg(["--import"], revocation);

const exported = (fpr) => text(gpg(["--export", "--armor", fpr]));
const encrypt = (recipients, extra = []) =>
  gpg([...at(SIGNED), "--encrypt", "--compress-algo", "none", "--cipher-algo", "AES256", ...recipients.flatMap((r) => ["-r", r]), ...extra], data);

writeFileSync(
  new URL("./pgp-vectors.json", import.meta.url),
  `${JSON.stringify(
    {
      data,
      fingerprints: { rsa, c25519, old, gone, other },
      keys: { rsa: exported(rsa[0]), c25519: exported(c25519[0]), old: exported(old[0]), gone: exported(gone[0]), other: exported(other[0]) },
      secret: { rsa: text(gpg(["--export-secret-keys", "--armor", rsa[0]])), c25519: text(gpg(["--export-secret-keys", "--armor", c25519[0]])) },
      signatures,
      messages: {
        to_rsa: b64(encrypt([rsa[0]])),
        to_c25519_armored: text(encrypt([c25519[0]], ["--armor"])),
        to_both: b64(encrypt([rsa[0], c25519[0]])),
      },
    },
    null,
    1,
  )}\n`,
);
try {
  execFileSync("gpgconf", ["--homedir", homedir, "--kill", "gpg-agent"], { stdio: "ignore" });
} catch {}
rmSync(home, { recursive: true, force: true });
console.log(`wrote pgp-vectors.json (${readdirSync(new URL(".", import.meta.url)).length} files beside it)`);
