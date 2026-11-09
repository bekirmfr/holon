#!/usr/bin/env bash
# A Postgres for CI's end-to-end tests: the ledger-watch sample's ledger (editor/e2e/server.mjs
# binds it), postgres:17.7 on 127.0.0.1:5433 with the login postgres/holon, in Docker, ready when
# this returns. On 5433, not 5432: a runner kept between runs may have a Postgres of its own.
set -euo pipefail
# With its volume: the image declares /var/lib/postgresql/data, and a kept runner would pile them up.
docker rm -f -v holon-postgres 2>/dev/null || true
docker run -d --name holon-postgres -p 127.0.0.1:5433:5432 -e POSTGRES_PASSWORD=holon postgres:17.7
# Ready once it answers on TCP: while the image sets the database up, it listens on its socket only.
for _ in $(seq 1 60); do
  docker exec holon-postgres pg_isready -U postgres -h 127.0.0.1 >/dev/null 2>&1 && exit 0
  sleep 1
done
echo "Postgres didn't start; its log:" >&2
docker logs holon-postgres 2>&1 | tail -40 >&2
exit 1
