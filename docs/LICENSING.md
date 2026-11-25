# Licensing

Everything about Holon's license in one place: the terms, where they live and ship, each choice
and why it was made, what is still missing, and what to check before each release. The terms
themselves are in [`../LICENSE`](../LICENSE); when this page and that file disagree, the file wins.

Set on 2026-11-25 (commit `dedb8a3`). The first release under these terms is the one after
0.1.0-rc.37.

## 1. The terms in short

Holon is **source-available**: anyone can read the code, and the license says who may use it for
what.

| Who | Terms |
|---|---|
| Anyone, for a non-commercial purpose (personal projects, study, hobby, charities, schools, research, public safety or health, environmental groups, government) | free |
| A small team: fewer than 30 people **and** under USD 1,000,000 revenue last tax year, the whole company with its affiliates counted | free, commercial use included |
| A larger organization | free to evaluate for 60 days, then a commercial license |
| A small team that grows past either limit | free for 90 more days, then a commercial license |
| Anyone reading, exporting or verifying run records (`holon export`, `holon verify`) | free, whatever their size |
| The flows people write, files Holon writes into their projects (`holon new`, `holon types`), the samples | theirs, to use under any terms |

By part:

| Part | License |
|---|---|
| `crates/holon-core` (the engine that evaluates flows) | Apache-2.0 |
| `crates/holon-module`, `fixtures/modules/js-engine` (the WebAssembly and JavaScript engine) | MIT or Apache-2.0 |
| `vendor/` (third-party code kept in the repository) | each its own |
| everything else, and the programs built from it (`holon`, the server, the web editor, the VS Code extension) | PolyForm Noncommercial 1.0.0 + Holon Additional Grant 1.0 |

## 2. Where the terms live, and where they ship

| File | What it holds |
|---|---|
| `LICENSE` | All of it in one file: the summary, which part is under what, the Additional Grant, how to get a commercial license, and the PolyForm Noncommercial 1.0.0 text, word for word as published |
| `LICENSE-APACHE` | The Apache License 2.0, as apache.org publishes it |
| `LICENSE-MIT` | The MIT License, with Holon's copyright line |
| `THIRD-PARTY-NOTICES.md` | The 700 outside packages Holon is built with, their licenses and license files |
| `.github/third-party-notices.mjs` | The script that writes the notices from the lockfiles |
| `README.md`, "License" | The table above, for readers |
| `CHANGELOG.md`, Unreleased | The change, for users |

What carries them:

- **Release archives** (cargo-dist). `LICENSE` is included automatically, as `holon`'s
  `license-file`. `LICENSE-APACHE`, `LICENSE-MIT` and `THIRD-PARTY-NOTICES.md` come in through
  `include` in `dist-workspace.toml` (paths relative to that file, each put at the archive's root).
  Not yet seen in a real archive: see §6.
- **The VS Code extension** (`holon-vscode.vsix`). `npm run package` in `editors/vscode` copies
  `LICENSE` and `THIRD-PARTY-NOTICES.md` from the root, and `.vscodeignore` lets them in. vsce
  renames `LICENSE` to `LICENSE.txt` inside the package. Checked once, by packaging it.
- **The web editor** is built into the `holon` binary, so the archive's files cover it.
- **Manifests.** `holon`, `holon-author`, `holon-host` and `holon-wasm` have
  `license-file = "../../LICENSE"`; `holon-core` has `license = "Apache-2.0"`; `holon-module`
  has `MIT OR Apache-2.0`. `editor/package.json` says `SEE LICENSE IN ../LICENSE`. No crate is
  published to crates.io (`publish = false`), so these fields only label the code.

The source itself is the public repository (`bekirmfr/holon`). Releases carry no source tarball
(`source-tarball = false`).

## 3. Choices and why

**Source-available, not open source.** An open-source license can't limit who uses the software
or for what, so a license that charges larger companies isn't one. Calling it open source would
draw public correction, which costs trust, the thing Holon sells. The README and the changelog say
"source-available".

**Paid by larger organizations, free below.** Holon's first buyers are payment-ops teams at
fintechs (`PLATFORM_LAYOUT.md` §7, M3). They are above both limits almost always, so they pay.
Individuals, hobby projects and small teams use it free, try it, find its problems and talk about
it. A license that let large companies use it free internally (FSL, the n8n licence) would leave
money only in paid extras.

**PolyForm Noncommercial as the base, with a grant on top.** Lawyers recognize a published
license and read a short addition to it quickly; custom license text slows a company's review.
PolyForm Small Business has its own limits (100 people, USD 1M) written in, and changing them
would make it a different license, so the base is PolyForm Noncommercial and the small-team rule
is a separate grant. The owner decided against a lawyer's review: the base license is already
reviewed, and the grant only adds permissions to it.

**Fewer than 30 people and under USD 1,000,000, both.** Each limit catches a different company:
the revenue limit, small companies that make money (12 people, USD 3M a year: they pay); the
headcount limit, funded startups with little revenue yet (40 people, USD 200k a year: they pay,
and can). Contractors count as well as employees, and the whole company counts with its
affiliates (PolyForm's own definition of "your company"), so a large company can't run it free
through a small subsidiary.

**The dollar limit is fixed.** It doesn't rise with inflation. Simpler to state and check; can
be raised in a later version of the grant.

**60 days to evaluate.** The person who tries Holon first, M3's outside user included, may well
work at a company above the limits. They need to try it without signing anything first.

**90 days after outgrowing the limits.** A team that grows should see the bill coming, not wake
up in breach. This is meant to be the moment a commercial license is sold.

**Use for a client counts as the client's.** Otherwise a 5-person agency could set up Holon for
a 200-person client and the client would never need a license.

**Checking records is free for everyone.** Holon's promise is that anyone can check a run record.
A fintech's auditor, bank partner or regulator must be able to run `holon verify` on records
they are given without a license of their own, whatever their size.

**Flows, generated files and samples are the user's.** The flows people write are their own
work, and the files Holon writes into a project (`holon new`'s starter, `holon.d.ts` and
`tsconfig.json` from `holon types`) and the samples people copy end up inside their projects.
None of it should carry Holon's license with it.

**Non-commercial organizations are free whatever their size.** This comes from PolyForm
Noncommercial itself (charities, schools, research, public safety and health, environmental
groups, government), and the grant leaves it as it is.

**The engine is Apache-2.0.** `holon-core` is what flows run on, what replay and verification
re-run, and what an editor or agent tool would build on. Giving it away widely helps Holon and
supports the "anyone can check" promise. `holon-module` (and the js engine module built on it)
already said MIT or Apache-2.0 and keeps it. Releasing code under Apache can't be taken back.

**Versions of the grant.** A later version applies to the releases made with it and takes away
nothing an earlier version gave for its releases. People can rely on the terms of the release
they have.

**Contact through GitHub.** The license names `https://github.com/bekirmfr` for a commercial
license, not an email, so no personal address is put in public files.

**Copyright holder.** `bekirmfr`, as before. For commercial contracts a legal name or a company
is cleaner (§5).

**Alternatives considered.**

| Option | Why not |
|---|---|
| Keep "all rights reserved" | Nobody could use a release without a written agreement, M3's tester included |
| Proprietary with a free-use grant, source closed | Closed developer tools are adopted slowly, and it weakens "anyone can check" |
| FSL or BSL (source-available, free internal use, no competing service) | Large companies would use it free internally; money only from extras |
| AGPL with paid licenses | Many companies, fintechs among them, ban AGPL outright |
| Apache-2.0 for everything | Anyone could host and resell it |

## 4. Third-party notices

`node .github/third-party-notices.mjs`, from the repository's root, writes
`THIRD-PARTY-NOTICES.md`. It needs the sources on disk first: `cargo fetch` at the root,
`cargo fetch` in `fixtures/modules/js-engine` (§5 on its lockfile), and
`npm ci --ignore-scripts` in `editor/` and `editors/vscode/`. It reaches the network only for
the standard license texts (below).

What it covers, each from its lockfile:

- the Rust crates `holon` and `holon-wasm` reach through normal dependencies, on every target
  (releases ship Linux, macOS and Windows); build and dev dependencies are left out;
- the crates of the JavaScript engine module, which the binary carries as `js-engine.wasm`;
- the npm packages of the web editor and of the VS Code extension's bundle (`package-lock.json`
  entries not marked dev).

How it writes them:

- each package once, by name and version, with its declared license and the license files it
  ships (their copyright lines are in them);
- identical texts, but for spacing, are written once and pointed to;
- a package that offers a choice (`MIT OR Apache-2.0`) and ships no license file is used under
  the first of MIT, Apache-2.0, BSD-3-Clause, BSD-2-Clause, ISC, Zlib, BSL-1.0, Unlicense it
  offers, and the standard text of that license (from the SPDX license list) is added, with the
  package's authors in the table.

As of 2026-11-25: 700 packages, 354 distinct license texts, about 1 MB. Every one is under a
permissive license (MIT, Apache-2.0, ISC, BSD, Zlib, Unicode-3.0, OFL-1.1 for the editor's fonts,
CDLA-Permissive-2.0, CC0, Unlicense, BSL-1.0). The only copyleft one offered is LGPL-2.1, by
`r-efi`, as a third choice beside MIT and Apache-2.0; it is used under MIT. 58 packages ship no
license file of their own (crates of a larger repository: oxc, boa, mongodb, objc2 and others).

## 5. Still missing

In order of what matters first.

1. **A commercial license to sell.** The grant sends larger organizations to a commercial
   license, but there is no agreement text and no price yet. The first evaluation that runs past
   60 days needs both. At least: a short order form over a standard agreement, a price per server
   or per deployment a year, and what it includes (support, the business features).
2. **A contact address.** Only the GitHub profile today. A buyer's procurement wants an email
   (a role address such as licensing@ on a domain keeps a personal one out of public files).
3. **Earlier releases.** 0.1.0-rc.1 to rc.37 were published under "all rights reserved", and the
   new terms are in the next release's `LICENSE`. Anyone running rc.37 today has, on paper, no
   license. Either cut a release soon, or say in the README that the new terms cover those
   releases too.
4. **Contributions from others.** There are none yet, and no contributor agreement. Before
   accepting the first outside pull request, add one (a CLA, or a copyright assignment) so their
   code can be sold under commercial licenses too. Without it, dual licensing stops at their
   code.
5. **The name.** The license covers the code, not the name "Holon". A short trademark note (who
   may call a fork or a service "Holon") would stop a hosted copy from using the name. Check the
   name is free to use as a mark in the markets that matter.
6. **The copyright holder.** "bekirmfr" is a GitHub handle. A legal name or a company is cleaner
   for contracts and for enforcing the license.
7. **The notices aren't checked by CI.** They change whenever a lockfile does, and nothing fails
   when they fall behind. A CI step could run the script and fail on a difference.
8. **The js engine's lockfile is behind its manifest.** `fixtures/modules/js-engine/Cargo.lock`
   lacks about 46 packages `holon-core` now needs (rsa, p256, ed25519 and others), so the
   committed `js-engine.wasm` was built with an older core, and `build.sh` would resolve them
   afresh. The script resolves the engine without `--locked` and puts the lockfile back. Rebuild
   the module and commit the lockfile with it, then drop the script's `stale` flag.
9. **Standard texts, not upstream ones, for 58 packages.** For a package that ships no license
   file, the notices give the standard text and the authors, not the copyright line of its
   project's own license file. Taking the file from each project's repository would be more
   exact.
10. **Holons shared through a marketplace.** The `marketplace` sample lets people publish holons.
    What license a published holon carries, and who may place it, isn't said. It doesn't matter
    until marketplaces are used (`PLATFORM_LAYOUT.md` §8).
11. **No license notice in source files.** Files carry no per-file header. Not required: the
    `LICENSE` file and the table in §1 say which part is under what.

## 6. Before each release

1. Regenerate the notices and commit them if they changed:

       cargo fetch
       (cd fixtures/modules/js-engine && cargo fetch && git checkout Cargo.lock)
       (cd editor && npm ci --ignore-scripts) && (cd editors/vscode && npm ci --ignore-scripts)
       node .github/third-party-notices.mjs
       git diff --stat THIRD-PARTY-NOTICES.md

2. Look at the license column for anything new that isn't permissive (GPL, AGPL, LGPL as the only
   choice, MPL, EPL, SSPL, BUSL, "not declared"). The script uses the first permissive choice a
   package offers; a package with no permissive choice needs a decision before it ships.
3. If the grant changed, raise its version in `LICENSE` (section 2's heading) and say what
   changed in `CHANGELOG.md`.
4. After the release, open one archive (for example `holon-x86_64-unknown-linux-gnu.tar.xz`) and
   check it holds `LICENSE`, `LICENSE-APACHE`, `LICENSE-MIT` and `THIRD-PARTY-NOTICES.md` beside
   the binaries. **Do this for the first release under these terms:** the `include` setting was
   written from cargo-dist's documentation and hasn't been seen working yet.
5. Open `holon-vscode.vsix` (a zip) and check it holds `extension/LICENSE.txt` and
   `extension/THIRD-PARTY-NOTICES.md`.

## 7. Changing the terms

- A change to the grant is a new version of it (`Holon Additional Grant, version 1.1`), in
  `LICENSE`, with a changelog entry. It covers the releases made with it; earlier releases keep
  theirs (grant §2.7).
- Raising a limit, lengthening a period or freeing another part can be done at any time.
  Narrowing one applies only to releases made afterward.
- Moving a part to Apache or MIT can't be undone for the releases it was made in.
- Keep the PolyForm text exactly as published: the base license is only recognized as itself.
- Update §1 and §3 of this page, the README's License section, and the table in `LICENSE` §1
  together.
