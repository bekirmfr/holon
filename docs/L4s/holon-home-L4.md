# One `~/.holon`: everything holon keeps, in one folder (L4)

**Status:** built (2026-10-25), with two departures from the design, marked *Built:* below. Where the `holon` program keeps its files, and a
`holon migrate` command that moves what older releases left elsewhere. No unit, hash or flow
changes. Holon is in development, so the migrations are a convenience, not a promise: an install
can also just delete what it had and start again.

---

## 1. The problem

`holon serve` without `--data` keeps its state in **whatever folder it is started from**, as
dot-files: `.holon-store`, `.holon-deployments.json`, `.holon-flows.json`,
`.holon-secrets.json`, `.holon-accounts.json`, `.holon-host-key`, and the files derived from them
(`.holon-deployments.logs/`, `…activity.jsonl`, `…heads/`, `.holon-secrets.audit.jsonl`), and it
reads `holon.toml` from there as its policy (main.rs, `serve`). Started from the home folder, as
it was on the machine this was written on, they all land in `~`. Started from a project, they
land in the project. The README works around it by telling people to pass
`--data ~/.holon/data`, and `uninstall --purge` assumes that folder, but nothing makes it the
default.

The rest is already mostly in `~/.holon`, each path built on its own:

| What | Now | Built in |
|---|---|---|
| The program | `~/.holon/bin` | dist-workspace.toml `install-path` |
| Run files of running servers | `~/.holon/run` (or `HOLON_RUN_DIR`) | local.rs `dir` |
| Tokens for other servers | `~/.holon/tokens.json` | local.rs `tokens_file` |
| MCP hosts set up | `~/.holon/mcp-hosts.toml` | mcphosts.rs |
| The secrets key, where there's no OS key store (Linux) | `~/.config/holon/keys/<id>.key` | sealing.rs `KeySource::parse` |
| The data `uninstall --purge` removes | `~/.holon/data` (or `--data`, `HOLON_DATA`) | selfmanage.rs |

`local.rs` and `selfmanage.rs` each have their own copy of "the home folder".

## 2. The layout

One root, **`~/.holon`** (`%USERPROFILE%\.holon` on Windows), or **`HOLON_HOME`** when set:

```
~/.holon/
  bin/              holon, and the holon-server alias          (the installer)
  data/             the server's state: the default for `holon serve`
    store/  flows.json  accounts.json  host.key  holon.toml
    secrets.json  secrets.audit.jsonl
    deployments.json  deployments.logs/  deployments.activity.jsonl  deployments.heads/
  keys/             secrets keys, where there's no OS key store
  run/              a file per running server
  tokens.json       tokens for servers elsewhere
  mcp-hosts.toml    the agent hosts `holon mcp setup` wrote to
```

- **One root, not each platform's folders.** XDG on Linux, `AppData` on Windows and `Library` on
  macOS would split this four ways per system (config, data, state, cache), which is the
  scattering this fixes. Developer tools mostly keep one folder (`~/.cargo`, `~/.rustup`,
  `~/.deno`). `HOLON_HOME` moves all of it, for a test, a second install or a service account.
- **`data/` is the one folder to back up, copy or move.** It holds everything a server needs and
  nothing about the machine. `keys/` is beside it, not inside, so a copied data folder doesn't
  carry the key that opens its secrets: the property the key's separate location has today.
- **The overrides stay, below the root.** `--data DIR` or `HOLON_DATA` replaces `data/`, each
  per-file flag (`--store`, `--secrets`, …) replaces its one file, and `HOLON_RUN_DIR` replaces
  `run/`. A flag always wins over the root.

**What stays outside**, because it isn't holon's to place:

- **The install receipt** (`%LOCALAPPDATA%\holon\holon-receipt.json`, `~/.config/holon/…`). The
  installer writes it and `holon update` reads it there; both are dist's.
- **Other applications' settings.** `holon mcp setup` writes into each agent host's own file
  (`~/.cursor/mcp.json`, …); that is the point of it.
- **The OS key store's entries** on Windows and macOS, under the service `holon`.

## 3. The changes

1. **One function for the root**, in local.rs beside `home()`: `holon_home()` is `HOLON_HOME`,
   else `home()/.holon`. Every path in §2 is built from it: `local::dir`, `local::tokens_file`,
   mcphosts.rs, the key file, `serve`'s data folder and `uninstall`. selfmanage.rs's own
   `home_dir` goes.
2. **`holon serve` keeps its state in `~/.holon/data` by default.** The data folder is
   `--data`, else `HOLON_DATA`, else `holon_home()/data`, so the `inside` step in `serve` always
   runs and the `.holon-*` defaults in the current folder are gone. The folder is created only
   if something lands in it, so a server given every file by flag (the dev launch config) makes
   nothing there. `serve` prints the folder it uses at start. The current folder no longer
   matters to `serve` at all, `holon.toml` included.
3. **The key goes with its file, not with where the file is.** Today the key's name in the key
   store (or its file name) is a hash of the secrets file's absolute path, so moving
   `secrets.json` anywhere leaves its secrets unreadable: the server looks for a key under a
   new name and finds none. Instead, the secrets file says which key it uses:
   - `SecretsFile` gains `key_id` (format 3). A new store takes a random id. The key is
     `secrets-key-<key_id>` in the OS key store, or `holon_home()/keys/<key_id>.key`.
     *Built:* a new store records the id of the path it is made at (the hash a format 2 file
     uses), not a random one. Recorded, it travels just the same, and a key made by an older
     release for a store not yet written is still found.
   - A format 2 file has no `key_id`: its id is the path hash, computed as today, and the next
     write records it as `key_id`. The key stays exactly where it is, under the same name, so
     nothing is re-filed and nothing is lost.
   - `key_check` already tells whether a key is the right one. Opening with a key that doesn't
     match it is refused, as now.
   - So a data folder can be moved, renamed or restored from a backup, and its secrets still
     open. A copy shares the key with the original, which is what a backup needs. The cost is
     that `uninstall --purge` of one copy removes the key the other uses too; purge says so
     when it asks.
4. **`uninstall --purge` removes all of `~/.holon`**: data, keys, run files, tokens and the MCP
   host list (it keeps telling the user which agent hosts' settings still name holon, and how to
   remove them: `holon mcp setup --remove`). Without `--purge` it keeps `~/.holon` and removes
   only the program, as now.
5. **Tests point `HOLON_HOME` at a temporary folder.** The e2e server (`e2e/server.mjs`) passes
   most files by flag but not `holon.toml`, which it reads from its working folder today and
   would read from the real `~/.holon/data` once the default moves. So it sets `HOLON_HOME`
   (and drops `HOLON_RUN_DIR`, now inside it). *Built:* it keeps `HOLON_RUN_DIR`, the folder
   the tests' fixtures (`e2e/fixtures.ts`) read both test servers' run files from; the dev
   launch config passes `--policy` too, so it reads nothing from the real home either. The Rust tests run the server in-process and
   don't start `serve`, but those that build home paths (local.rs, mcphosts.rs, selfmanage.rs)
   set `HOLON_HOME` too. No test may read or write the real home.

## 4. `holon migrate`

One command for what an older release left that this one keeps elsewhere, and the place later
changes of the same kind go:

```
holon migrate            # say what would be done, then ask
holon migrate --yes      # do it without asking
holon migrate --check    # exit 1 if anything needs migrating, change nothing
```

A migration is two functions: `needed(&Env) -> Option<Plan>`, which looks and changes nothing,
and `apply(Plan) -> Result<Report>`. `holon migrate` runs every `needed`, prints each plan in a
line or two, asks once, and applies them in order. It is idempotent: once applied, `needed` finds
nothing, so running it twice is harmless. It never deletes anything it hasn't first copied and
checked, and it refuses to run while a server is running (a run file in `run/` whose server
still answers `/health`, as local.rs tells a stale one apart), since nothing locks the data
files. A server too old to write run files can't be seen this way; the plan says to stop any.

**The first two:**

- **`data-in-a-folder`: a server's state left as `.holon-*` files.** Found in the current folder,
  or in the folder given as `holon migrate --from DIR`, when `~/.holon/data` doesn't hold a
  server yet. Applying copies each file under its new name into `data.partial/`, checks that the
  store loads and that the secrets open with their key, renames `data.partial/` to `data/`, then
  removes the old files. If the secrets can't be opened (no key, or not the right one), it stops
  and says so before anything moves. Interrupted before the rename, the old files are untouched
  and the next run starts over. Old name to new name:

  | Old | New, in `data/` |
  |---|---|
  | `.holon-store/` | `store/` |
  | `.holon-deployments.json` (`.logs/`, `.activity.jsonl`, `.heads/`) | `deployments.json` (and the same) |
  | `.holon-flows.json` | `flows.json` |
  | `.holon-secrets.json` (`.audit.jsonl`) | `secrets.json` (and the same) |
  | `.holon-accounts.json` | `accounts.json` |
  | `.holon-host-key` | `host.key` |
  | `holon.toml`, when the others are there | `holon.toml` |

  Because of §3.3 the secrets file keeps its key under its old id. A format 2 file is written
  as format 3 with `key_id` set to the id its old path gave, so the key isn't touched.
- **`keys-in-config`: a key file in `~/.config/holon/keys/`** (Linux). Moved to `~/.holon/keys/`
  under the same name, owner-only, and the empty folders it leaves are removed.

**When `serve` finds the old files.** Started in a folder that holds `.holon-*` files, it starts
on `~/.holon/data` as usual and prints one line: that folder holds an older server's data, and
`holon migrate` moves it. It doesn't refuse and doesn't migrate on its own: during development
the old data is as likely to be stale as wanted.

## 5. What doesn't change

- **Hashes, units, flows, the language.** Nothing a flow does or is depends on where the host
  keeps its files.
- **Explicit paths.** A server started with `--data DIR` or every file by flag keeps using
  exactly those.
- **The run file, the token file and the MCP host list**: their contents and their places, which
  were already under `~/.holon`.

There is no sample flow for this: it is where the host keeps its files, which no flow can show.
The tests of §6 stand in for it.

## 6. Tests

- **The root.** With `HOLON_HOME` set, every path of §2 is under it; without it, under
  `home()/.holon`. `--data`, `HOLON_DATA`, `HOLON_RUN_DIR` and the per-file flags each still win.
- **`serve`'s default.** Started in an empty folder with `HOLON_HOME` set, it writes nothing in
  that folder and its state in `HOLON_HOME/data`. Started with every file by flag, it makes no
  `data/`.
- **The key travels.** A format 2 store, opened, is written back as format 3 with the path's id;
  the store moved to another folder still opens its secrets; a new store records its path's id;
  a key that doesn't match `key_check` is refused.
- **`holon migrate`.** For each migration: `needed` finds the old layout and nothing once it's
  applied; `apply` moves every file of the table and the server starts on the result with its
  accounts, flows, deployments, runs and secrets; `--check` exits 1 then 0; it refuses while a
  run file is live; a store whose secrets don't open stops it with the old files untouched; an
  interruption before the rename leaves them untouched too.
- **`uninstall --purge`** removes `~/.holon` and the key, and names the agent hosts still set
  up.
- **e2e**: the servers run under a temporary `HOLON_HOME`. One test checks the real home isn't
  touched by giving `HOME`/`USERPROFILE` a folder of their own and finding it empty afterwards.
- **Live, once**: `holon migrate` on a copy of a home folder with the old files (the one this
  was written on has them), then `holon serve` on the result signs in, lists its flows and runs
  a deployment that reads a secret.

## 7. What ships with it

- local.rs `holon_home()`, and every path of §2 built from it.
- `serve`'s default data folder; the line it prints for old files.
- `SecretsFile` format 3 with `key_id`; keys under `holon_home()/keys`.
- `holon migrate` with `data-in-a-folder` and `keys-in-config`; its usage in `holon --help`.
- `uninstall --purge` removing `~/.holon`.
- `HOLON_HOME` in the e2e server and the server tests.
- Docs: the README's Running section (`holon serve`, no `--data` needed), `uninstall`, a
  CHANGELOG entry pointing at `holon migrate`; PLATFORM_LAYOUT.md's line on `--data`.
