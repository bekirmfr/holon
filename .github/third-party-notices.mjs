#!/usr/bin/env node
// Writes THIRD-PARTY-NOTICES.md: the third-party software Holon's releases are built with, and
// the license each comes under, with the license files each package ships (their copyright lines
// in them). Releases carry it beside LICENSE (dist-workspace.toml's `include`), and so does the
// VS Code extension (editors/vscode's `npm run package`).
//
// What it covers, each from its lockfile:
// - the Rust crates the holon binary and the editor's wasm core link (Cargo.lock: holon's and
//   holon-wasm's normal dependencies, every target; build and dev dependencies left out);
// - the crates of the JavaScript engine module (fixtures/modules/js-engine/Cargo.lock), which the
//   binary carries as js-engine.wasm;
// - the npm packages the web editor (built into the binary) and the VS Code extension bundle
//   (package-lock.json entries not marked dev).
//
// Run from the repository's root once the sources are on disk: `cargo fetch` (here and in
// fixtures/modules/js-engine), and `npm ci --ignore-scripts` in editor/ and editors/vscode/. Run it
// again whenever a lockfile changes:
//
//     node .github/third-party-notices.mjs
//
// A package that ships no license file is listed with the license its manifest declares.

import { execFileSync } from "node:child_process"
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"

const ROOT = process.cwd()
const LICENSE_FILE = /^(licen[cs]e|copying|copyright|notice|unlicense)([-._].*)?$/i

// Each package once, by name and version: { name, version, license, from, files: [[name, text]] }.
const packages = new Map()

function licenseFiles(dir, declared) {
  const files = []
  const names = existsSync(dir) ? readdirSync(dir).filter((f) => LICENSE_FILE.test(f)).sort() : []
  if (declared && !names.includes(declared)) names.push(declared)
  for (const f of names) {
    const p = join(dir, f)
    if (existsSync(p) && statSync(p).isFile()) files.push([f, readFileSync(p, "utf8").replace(/\r\n/g, "\n").trim()])
  }
  return files
}

function add(key, entry) {
  if (!packages.has(key)) packages.set(key, entry)
  else if (!packages.get(key).from.includes(entry.from[0])) packages.get(key).from.push(entry.from[0])
}

// The crates `roots` reach through normal dependencies, in the workspace at `dir`. With `stale`,
// the workspace's Cargo.lock may be behind its manifests (the engine module's, rebuilt by hand:
// fixtures/modules/js-engine/build.sh), so cargo resolves it afresh and the lockfile is put back
// as it was.
function crates(dir, roots, from, stale = false) {
  const lockfile = join(dir, "Cargo.lock")
  const kept = readFileSync(lockfile)
  let meta
  try {
    meta = JSON.parse(
      execFileSync("cargo", ["metadata", "--format-version", "1", "--offline", ...(stale ? [] : ["--locked"])], { cwd: dir, maxBuffer: 1 << 28 }),
    )
  } finally {
    writeFileSync(lockfile, kept)
  }
  const byId = new Map(meta.packages.map((p) => [p.id, p]))
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]))
  const own = new Set(meta.workspace_members)
  const seen = new Set()
  const stack = meta.packages.filter((p) => roots.includes(p.name) && own.has(p.id)).map((p) => p.id)
  if (stack.length !== roots.length) throw new Error(`${dir}: not all of ${roots} are workspace members`)
  while (stack.length) {
    const id = stack.pop()
    if (seen.has(id)) continue
    seen.add(id)
    for (const d of nodes.get(id).deps) {
      if (d.dep_kinds.some((k) => k.kind === null)) stack.push(d.pkg)
    }
  }
  for (const id of seen) {
    const p = byId.get(id)
    const dir = dirname(p.manifest_path)
    // Holon's own crates (in this repository, but for vendor/) are under LICENSE.
    if (dir.startsWith(ROOT + "/") && !dir.startsWith(join(ROOT, "vendor") + "/")) continue
    add(`${p.name}@${p.version}`, {
      name: p.name,
      version: p.version,
      license: p.license ?? (p.license_file ? `see ${p.license_file}` : "not declared"),
      from: [from],
      authors: p.authors ?? [],
      files: licenseFiles(dir, p.license_file),
    })
  }
}

// The npm packages a lockfile installs for running, not developing.
function npm(dir, from) {
  const lock = JSON.parse(readFileSync(join(dir, "package-lock.json"), "utf8"))
  for (const [path, p] of Object.entries(lock.packages)) {
    if (path === "" || p.dev || p.link) continue
    const at = join(dir, path)
    if (!existsSync(join(at, "package.json"))) throw new Error(`${at} is not installed: run npm ci --ignore-scripts in ${dir}`)
    const manifest = JSON.parse(readFileSync(join(at, "package.json"), "utf8"))
    const name = manifest.name ?? path.replace(/^.*node_modules\//, "")
    const license = typeof manifest.license === "string" ? manifest.license : manifest.license?.type ?? p.license ?? "not declared"
    const author = typeof manifest.author === "string" ? manifest.author : manifest.author?.name
    add(`${name}@${manifest.version}`, { name, version: manifest.version, license, from: [from], authors: author ? [author] : [], files: licenseFiles(at) })
  }
}

crates(ROOT, ["holon", "holon-wasm"], "the holon program and the editor's core")
crates(join(ROOT, "fixtures/modules/js-engine"), ["holon-js-engine"], "the JavaScript engine module", true)
npm(join(ROOT, "editor"), "the web editor")
npm(join(ROOT, "editors/vscode"), "the VS Code extension")

const list = [...packages.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version, undefined, { numeric: true }))

// License files the same but for spacing are written once, and each package points to the text
// it ships.
const texts = new Map()
const same = (text) => text.replace(/\s+/g, " ")
for (const p of list) {
  p.refs = p.files.map(([, text]) => {
    const key = same(text)
    if (!texts.has(key)) texts.set(key, { n: texts.size + 1, text, users: [] })
    texts.get(key).users.push(`${p.name} ${p.version}`)
    return texts.get(key).n
  })
}

// A package that ships no license file is under the license its manifest declares, kept in its
// project elsewhere (a crate of a larger repository, often): the standard text of each license
// named so, from the SPDX license list, with the package's authors beside it in the table.
// Where the manifest offers a choice (`MIT OR Apache-2.0`), the package is used under the first
// of PREFERRED it offers.
const PREFERRED = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib", "BSL-1.0", "Unlicense"]
function usedUnder(expr) {
  const choices = expr.replace(/[()]/g, "").split(/\s+OR\s+|\//).map((c) => c.trim())
  return PREFERRED.find((id) => choices.includes(id)) ?? choices[0]
}
const bare = list.filter((p) => !p.files.length)
for (const p of bare) p.under = usedUnder(p.license)
const ids = [...new Set(bare.flatMap((p) => p.under.split(/\s+(?:AND|WITH)\s+/).map((s) => s.trim()).filter((s) => /^[A-Za-z0-9.+-]+$/.test(s))))].sort()
const standard = []
for (const id of ids) {
  const url = `https://raw.githubusercontent.com/spdx/license-list-data/main/text/${id}.txt`
  const got = await fetch(url)
  if (!got.ok) throw new Error(`no standard text for ${id} (${url}: ${got.status})`)
  standard.push([id, (await got.text()).replace(/\r\n/g, "\n").trim()])
}

const out = []
out.push("# Third-party notices", "")
out.push("Holon's releases are built with the third-party software below. Each is used under its own")
out.push("license, given here as its package declares it, with the license files the package ships.")
out.push("Holon's own terms are in LICENSE.", "")
out.push("Written by `.github/third-party-notices.mjs` from the lockfiles: run it again when they change.", "")
out.push(`## Packages (${list.length})`, "")
out.push("| Package | Version | License | In | License files |", "|---|---|---|---|---|")
for (const p of list) {
  const refs = p.refs.length
    ? p.refs.map((n) => `[${n}](#text-${n})`).join(", ")
    : `none shipped: used under ${p.under}, its standard text below${p.authors.length ? `; authors: ${p.authors.join(", ").replace(/[<>]/g, "")}` : ""}`
  out.push(`| ${p.name} | ${p.version} | ${p.license.replace(/\|/g, "\\|")} | ${p.from.join("; ")} | ${refs} |`)
}
out.push("", `## License texts (${texts.size})`, "")
for (const { n, text, users } of texts.values()) {
  out.push(`<a id="text-${n}"></a>`, `### ${n}`, "", `Shipped by: ${users.join(", ")}`, "", "````text", text, "````", "")
}
out.push(`## Standard license texts (${standard.length})`, "")
out.push("For the packages above that ship no license file, the standard text of each license they declare.", "")
for (const [id, text] of standard) out.push(`### ${id}`, "", "````text", text, "````", "")
writeFileSync(join(ROOT, "THIRD-PARTY-NOTICES.md"), out.join("\n"))
console.log(`THIRD-PARTY-NOTICES.md: ${list.length} packages, ${texts.size} license texts; ${bare.length} ship no license file (standard texts: ${ids.join(", ")})`)
