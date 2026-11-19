# Running it as a service

`holon serve` runs in the foreground until it is told to stop. On a server you hand it to what
starts programs there: systemd on Linux, a container runtime, or a service wrapper on Windows.
Four things are the same everywhere:

- **Where the root key is.** The root key opens everything the server seals
  ([the root key](../reference/server.md#the-root-key)). A service runs as an account of its own,
  which doesn't see the OS key store of the person who set it up: give it `--root-key file:PATH`
  (a file only the service's account reads, outside the data folder) or `--root-key env:VAR`
  (from the service manager's secrets). On Linux the default is already a file,
  `~/.holon/keys/<id>.key` in the service account's home. Back it up apart from the data folder.
- **How long a stop takes.** Told to stop (SIGTERM, ctrl+c), the server starts no more runs,
  waits for those going on for up to `--stop-within` (default `1m`), syncs the logs, and exits
  ([stopping it](../reference/server.md#stopping-it)). The service manager's own wait must be
  longer, or it kills the server in the middle of a sync: give it the server's wait and a margin.
  A run still going on at the end of `--stop-within` is cut off as a crash cuts it, and the next
  start says which deployments it may have left part-done.
- **What `/health` says.** `GET /health` answers `ok`, or 503 `not ok: ` and what fails: the
  deployments file can't be written, or the run logs can't be synced to disk. It answers 503 while
  the server stops too. It is a readiness check: a load balancer sends requests elsewhere while it
  fails. Don't restart the server on it: a restart doesn't make room on a disk, and a deploy that
  couldn't be written is lost by it. The admin's warnings say why, and what to do
  ([warnings](../reference/server.md#warnings)). The control plane answers only to a Host it
  knows, so a check from outside the machine sends `Host: localhost:<port>` or uses a name given
  to `--allow-host`.
- **Who may reach it.** It speaks plain HTTP; TLS ends at a proxy in front of it
  ([a public HTTPS address](public-https.md)). Keep `--at` on loopback unless the proxy is on
  another machine.

## systemd (Linux)

A system account of its own, its data folder, and a unit:

```bash
sudo useradd --system --create-home --home-dir /var/lib/holon holon
sudo install -d -o holon -g holon -m 700 /var/lib/holon/data
```

```ini
# /etc/systemd/system/holon.service
[Unit]
Description=Holon
After=network-online.target
Wants=network-online.target

[Service]
User=holon
ExecStart=/usr/local/bin/holon serve --data /var/lib/holon/data --at 127.0.0.1:7420
# The server waits up to --stop-within (1m) for the runs going on, then syncs: wait longer.
TimeoutStopSec=90
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now holon
journalctl -u holon
```

The root key is made at the first start in `/var/lib/holon/.holon/keys/`, outside the data
folder. The first start prints the setup code for the first admin in the journal (or give it one
in `HOLON_SETUP_CODE`), and, until an admin chooses where the runs each flow's log lets go of go,
a warning that they are deleted: choose with `holon policy --backup DIR` (a second drive, say) or
`--backup none` ([Warnings](../reference/server.md#warnings)). systemd sends SIGTERM to stop it, then SIGKILL once `TimeoutStopSec` has
passed.

## A container

Holon's Linux builds use glibc: a Debian or Ubuntu base, not Alpine. The data folder is a volume;
the root key comes from the runtime's secrets, never from the image:

```dockerfile
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* && useradd --system --create-home holon
COPY holon /usr/local/bin/holon
USER holon
VOLUME /data
EXPOSE 7420
HEALTHCHECK CMD curl -fsS http://127.0.0.1:7420/health || exit 1
ENTRYPOINT ["holon", "serve", "--data", "/data", "--at", "0.0.0.0:7420", "--root-key", "env:HOLON_ROOT_KEY"]
```

`holon serve --new-root-key` prints a key for `HOLON_ROOT_KEY`; keep it in the runtime's secrets
and a copy apart. Add `--allow-host` for the name the proxy forwards, and `--data-host 0.0.0.0`
for flows' own ports to be reached from outside the container.

Give the stop longer than the server's wait: Docker's default is 10 seconds
(`docker run --stop-timeout 70`, or `stop_grace_period: 70s` in a Compose file). In Kubernetes:
`terminationGracePeriodSeconds: 70`; a `readinessProbe` on `/health` with the header
`Host: localhost:7420`; and a `livenessProbe` on the TCP port, not on `/health`, so a disk that
fails doesn't restart the server in a loop. The server handles SIGTERM itself, as the first
process in the container too.

## Windows

`holon serve` is a console program, not a Windows service. Run it under a service wrapper that
starts it at boot and stops it with ctrl+c, such as [NSSM](https://nssm.cc):

```powershell
nssm install holon C:\holon\holon.exe serve --data D:\holon\data --root-key file:D:\holon-key\root.key
nssm set holon AppStopMethodConsole 70000
nssm start holon
```

`AppStopMethodConsole` is how long NSSM waits after its ctrl+c (1.5 seconds unless set): give it
longer than `--stop-within`. A service's account (LocalSystem, or one of its own) doesn't see the
Credential Manager of the person who set it up, so the root key is in a file only that account
reads (`icacls` to take the others out), or in `env:VAR`. A scheduled task that runs `holon serve`
at startup works too; closing its console gives the server a few seconds only.
