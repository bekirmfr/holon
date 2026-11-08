// Test vectors for holon-core's jwt.rs, made by Node's crypto (OpenSSL): keys of each kind, and
// tokens signed with them, valid and not. Printed as JSON.
import { constants, createHmac, generateKeyPairSync, sign } from "node:crypto";

const b64u = (b) => Buffer.from(b).toString("base64url");
const enc = (o) => b64u(JSON.stringify(o));
const rsa = generateKeyPairSync("rsa", { modulusLength: 2048 });
const weak = generateKeyPairSync("rsa", { modulusLength: 1024 });
const ec = generateKeyPairSync("ec", { namedCurve: "P-256" });
const ed = generateKeyPairSync("ed25519");
const secret = "a shared secret of thirty-two by"; // 32 bytes
const long = "x".repeat(64);

const claims = { iss: "partner-a", sub: "svc-1", aud: "payouts-api", scope: "payouts:write", iat: 1794150000, nbf: 1794150000, exp: 1794153600 };
const jws = (header, payload, signer) => {
  const signed = `${enc(header)}.${enc(payload)}`;
  return `${signed}.${b64u(signer(Buffer.from(signed)))}`;
};
const hashOf = { 256: "sha256", 384: "sha384", 512: "sha512" };
const rs = (bits, key = rsa.privateKey) => (d) => sign(hashOf[bits], d, key);
const ps = (bits) => (d) => sign(hashOf[bits], d, { key: rsa.privateKey, padding: constants.RSA_PKCS1_PSS_PADDING, saltLength: bits / 8 });
const es = (d) => sign("sha256", d, { key: ec.privateKey, dsaEncoding: "ieee-p1363" });
const eddsa = (d) => sign(null, d, ed.privateKey);
const hs = (bits, key) => (d) => createHmac(hashOf[bits], key).update(d).digest();

const pem = (k, type = "spki") => k.publicKey.export({ type, format: "pem" });
const jwk = (k, extra) => ({ ...k.publicKey.export({ format: "jwk" }), ...extra });
const rsaPem = pem(rsa);

const tokens = {};
for (const bits of [256, 384, 512]) {
  tokens[`RS${bits}`] = jws({ alg: `RS${bits}`, typ: "JWT", kid: "rsa-1" }, claims, rs(bits));
  tokens[`PS${bits}`] = jws({ alg: `PS${bits}`, kid: "rsa-1" }, claims, ps(bits));
}
tokens.HS256 = jws({ alg: "HS256", typ: "JWT" }, claims, hs(256, secret));
tokens.HS384 = jws({ alg: "HS384" }, claims, hs(384, long));
tokens.HS512 = jws({ alg: "HS512" }, claims, hs(512, long));
tokens.ES256 = jws({ alg: "ES256", kid: "ec-1" }, claims, es);
tokens.EdDSA = jws({ alg: "EdDSA", kid: "ed-1" }, claims, eddsa);
tokens.Ed25519 = jws({ alg: "Ed25519", kid: "ed-1" }, claims, eddsa);
// What must give nothing.
tokens.none = `${enc({ alg: "none" })}.${enc(claims)}.`;
tokens.confused = jws({ alg: "HS256" }, claims, hs(256, rsaPem)); // the RSA public key as the HMAC secret
tokens.weak = jws({ alg: "RS256" }, claims, rs(256, weak.privateKey));
tokens.crit = jws({ alg: "RS256", kid: "rsa-1", crit: ["exp"] }, claims, rs(256));
const { exp, ...forever } = claims;
tokens.no_exp = jws({ alg: "RS256", kid: "rsa-1" }, forever, rs(256));
tokens.aud_list = jws({ alg: "RS256", kid: "rsa-1" }, { ...claims, aud: ["other", "payouts-api"] }, rs(256));
tokens.exp_text = jws({ alg: "RS256", kid: "rsa-1" }, { ...claims, exp: "1794153600" }, rs(256));
tokens.other_kid = jws({ alg: "RS256", kid: "rsa-2" }, claims, rs(256));
tokens.no_kid = jws({ alg: "RS256" }, claims, rs(256));
tokens.short_secret = jws({ alg: "HS256" }, claims, hs(256, "too short"));
tokens.padded = tokens.RS256.replace(/\.([^.]*)$/, (m, s) => `.${s}${"=".repeat((4 - (s.length % 4)) % 4) || "=="}`);
tokens.array_payload = jws({ alg: "RS256", kid: "rsa-1" }, [1, 2], rs(256));

console.log(JSON.stringify({
  claims,
  secret,
  long,
  rsa_pem: rsaPem,
  rsa_pkcs1_pem: pem(rsa, "pkcs1"),
  rsa_jwk: jwk(rsa, { kid: "rsa-1", use: "sig" }),
  weak_pem: pem(weak),
  ec_pem: pem(ec),
  ec_jwk: jwk(ec, { kid: "ec-1", alg: "ES256" }),
  ed_pem: pem(ed),
  ed_jwk: jwk(ed, { kid: "ed-1" }),
  tokens,
}, null, 1));
