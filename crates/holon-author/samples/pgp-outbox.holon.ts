// PGP outbox: payment files encrypted to the bank's key, and the bank's signed files verified.
//
// A treasury team sends payment files to its bank over SFTP, and the bank asks for them encrypted
// to its OpenPGP key; its status reports come back signed with that key. Each file dropped in
// payments is encrypted to the bank's key valid now and written, armored, to upload, where the
// SFTP job picks it up; it is kept with its SHA-256, the encrypted file's, and the key it went
// to. Each report the bank delivers to download with its detached signature (a .sig beside it) is
// verified with the bank's key: kept as verified, with the key that signed it and when, or as
// refused. The bank's key is one a person trusts, by running the trust action, which answers its
// fingerprint to compare with the one the bank gave; an agent can't trust a key on its own.
//
// Before it works, an admin binds the folders:
//   holon folder add payments ~/holon-payments --read-only
//   holon folder add upload ~/holon-upload
//   holon folder add download ~/holon-download --read-only
//
// Try it: deploy it, run trust from the editor's server panel with the bank's public key,
// {"key": "-----BEGIN PGP PUBLIC KEY BLOCK-----…"}, and compare the fingerprint it answers. Then
//   cp pain.001.xml ~/holon-payments/             encrypted to ~/holon-upload/pain.001.xml.asc
//   gpg --decrypt ~/holon-upload/pain.001.xml.asc  what the bank reads, with its secret key
//   cp pain.002.xml pain.002.xml.sig ~/holon-download/
//   curl http://127.0.0.1:8185/sent                each payment file sent, and to which key
//   curl http://127.0.0.1:8185/received            each report, verified or refused
import { action, flow, on, pgp_encrypt, pgp_key, pgp_verify, route, sha256 } from "holon"
import { now } from "holon:clock"
import { random_bytes } from "holon:random"
import state from "state"
import received from "collection:received"
import sent from "collection:sent"
import download from "folder:download"
import payments from "folder:payments"
import upload from "folder:upload"

flow("pgp-outbox", { port: 8185 })

// The bank's public key, trusted by a person, kept with its fingerprint; nothing kept for what
// isn't one. It answers the fingerprint, its user IDs, when it expires and whether it is revoked.
export const trust = action((input) => {
  try {
    const key = pgp_key(input.key)
    state.bank = { key: input.key, fingerprint: key.fingerprint }
    return {
      fingerprint: key.fingerprint,
      user_ids: key.user_ids,
      expires: key.expires ?? "never",
      revoked: key.revoked,
    }
  } catch {
    return { refused: "not one OpenPGP public key its own signatures hold for" }
  }
})

// A payment file: encrypted to the bank's key, armored, for the SFTP job to upload.
on.file.created(payments, (f) => {
  try {
    const data = payments.bytes(f.name)
    const encrypted = pgp_encrypt(data, state.bank.key, random_bytes, now, { armor: "ascii" })
    upload[f.name + ".asc"] = encrypted
    sent[f.name] = {
      to: state.bank.fingerprint,
      sha256: sha256(data),
      encrypted: sha256(encrypted),
      at: now,
    }
  } catch {
    sent[f.name] = { refused: "no bank key is trusted, or none of its keys encrypts now" }
  }
})

// A report's signature, beside the report: verified with the bank's key.
on.file.created(
  download,
  (f) => {
    const report = f.name.slice(0, -4)
    try {
      const data = download.bytes(report)
      const signed = pgp_verify(data, download.bytes(f.name), state.bank.key, now)
      received[report] = { verified: signed.key, signed: signed.created, sha256: sha256(data) }
    } catch {
      received[report] = { refused: "not signed by the bank's trusted key, or not there" }
    }
  },
  { matching: "*.sig" },
)

route.get("/sent", () => sent.list())

route.get("/received", () => received.list())
