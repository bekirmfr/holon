# holon

Build services by wiring small, pure parts together, deploy them with exactly the permissions
they need, and see every run as it happened.

## Installing

Each release (GitHub Releases) holds `holon-server` for Linux (x64), macOS (Apple silicon, Intel)
and Windows (x64), with the editor built in, their checksums, and install scripts that fetch the
right one, check it, and put it in `~/.holon/bin`, on your PATH.

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bekirmfr/holon/releases/latest/download/holon-server-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/bekirmfr/holon/releases/latest/download/holon-server-installer.ps1 | iex"
```

While the repository is private, the install scripts can't download its releases (GitHub
serves private release files only through its API, even with a token). Install by hand with the
GitHub CLI, signed in with access to the repository: download your platform's archive and its
checksum, check it, and unpack `holon-server` onto your PATH:

```sh
gh release download --repo bekirmfr/holon --pattern 'holon-server-x86_64-unknown-linux-gnu.tar.xz*'
sha256sum -c holon-server-x86_64-unknown-linux-gnu.tar.xz.sha256
tar -xJf holon-server-x86_64-unknown-linux-gnu.tar.xz
```

(The archives: `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-pc-windows-msvc.zip`.) The scripts' settings: `HOLON_SERVER_INSTALL_DIR` (where),
`HOLON_SERVER_NO_MODIFY_PATH=1` (leave PATH alone).

## Running

```sh
holon-server --data ~/.holon/data
```

Then open http://127.0.0.1:7420. The first time, the server prints a one-time setup code: enter
it in the editor to create the admin. `--data` keeps all of the server's state in one folder.
To reach it from other machines, see `--allow-host` and `HOLON_SERVER_ADDR` in
`docs/PLATFORM_LAYOUT.md`, and put HTTPS in front (a reverse proxy).

## Releasing

Releases are built by `.github/workflows/release.yml` (cargo-dist; its config is
`dist-workspace.toml`). To release: set the version in `crates/holon-server/Cargo.toml`, commit,
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
`cargo run -p holon-server`; the editor: `npm --prefix editor run dev`; tests: `cargo test`,
`npm --prefix editor test`, `npm --prefix editor run e2e`.
