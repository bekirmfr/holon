# holon

Build services by wiring small, pure parts together, deploy them with exactly the permissions
they need, and see every run as it happened.

## Installing

Each release (GitHub Releases) holds `holon` for Linux (x64, arm64), macOS (Apple silicon, Intel)
and Windows (x64), with the editor built in, their checksums, and install scripts that fetch the
right one, check it, and put it in `~/.holon/bin`, on your PATH. One command does everything:
`holon serve` runs the server, `holon new`/`check`/`show`/`fmt` work on flows written as text,
and `holon version`/`update`/`uninstall` look after the install.

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.sh | sh
```

Windows, in PowerShell:

```powershell
irm https://github.com/bekirmfr/holon/releases/latest/download/holon-installer.ps1 | iex
```

(Not the `powershell -ExecutionPolicy Bypass -c "irm … | iex"` form some tools suggest: Microsoft
Defender flags that command line as a trojan pattern and blocks it. If an antivirus stops the
script anyway, download `holon-x86_64-pc-windows-msvc.zip` from the release and put `holon.exe`
somewhere on your PATH; `update` then can't replace it, but everything else works.)

`latest` is the newest full release; GitHub skips pre-releases there. For a pre-release (or any
version), name it: `…/releases/download/v0.1.0-rc.4/holon-installer.sh`. The scripts' settings:
`HOLON_INSTALL_DIR` (where), `HOLON_NO_MODIFY_PATH=1` (leave PATH alone).

Releases up to 0.1.0-rc.3 installed it as `holon-server`. `holon-server update` on one of those
installs `holon` in its place (each release also carries its installers under the old name for
this), and keeps a `holon-server` beside it for scripts that start the server by that name: it
runs `holon serve`.

To check a download came from this repository's release workflow (build provenance, from
v0.1.0-rc.2 on):

```sh
gh attestation verify holon-x86_64-unknown-linux-gnu.tar.xz --repo bekirmfr/holon
```

Once installed:

```sh
holon version            # this build's version
holon update --check     # is there a newer release?
holon update             # install it over this one (--to 0.1.0 for a given version)
holon uninstall          # remove the program, its PATH entry and install receipt
```

A pre-release build follows pre-releases; a full release, only full releases (`--pre` to include
them). `update` works on copies the install scripts put in place (they leave a receipt saying
where); a copy unpacked by hand is replaced by hand. `uninstall` keeps the server's data;
`uninstall --purge` removes the data folder too (`--data DIR`, else `HOLON_DATA`, else
`~/.holon/data`) and its secrets key. It asks first; `--yes` skips that.

## Running

```sh
holon serve --data ~/.holon/data
```

Then open http://127.0.0.1:7420. The first time, the server prints a one-time setup code: enter
it in the editor to create the admin. `--data` keeps all of the server's state in one folder.
To reach it from other machines, see `--allow-host` and `HOLON_SERVER_ADDR` in
`docs/PLATFORM_LAYOUT.md`, and put HTTPS in front (a reverse proxy).

Flows can also be written as text (`.holon` files), on any machine, a screen or not:

```sh
holon new my-flow.holon                  # a starter flow (--sample hello-api for a sample)
holon check my-flow.holon                # its routes, what it may do, or each mistake by line
holon show my-flow.holon                 # an outline: routes, nodes and what feeds them
holon fmt my-flow.holon                  # rewrite it in the canonical form
```

## Releasing

Releases are built by `.github/workflows/release.yml` (cargo-dist; its config is
`dist-workspace.toml`). To release: set the version in `crates/holon/Cargo.toml`, commit,
then push a tag for it:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

`dist plan` shows what a release holds; `dist build --artifacts=local` builds this machine's
archive (it needs the editor built first: `npm --prefix editor run build`).

## License

Proprietary: all rights reserved (see `LICENSE`). No use, copying or distribution without a
written commercial agreement.

## Developing

See `CLAUDE.md` for the conventions and `docs/` for the design. The server:
`cargo run -p holon -- serve`; the editor: `npm --prefix editor run dev`; tests: `cargo test`,
`npm --prefix editor test`, `npm --prefix editor run e2e`.
