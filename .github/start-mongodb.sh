#!/usr/bin/env bash
# A MongoDB for CI's tests: a one-node replica set on 127.0.0.1:27017 (a commit is a transaction,
# which needs one), in Docker, ready to write when this returns.
set -euo pipefail
docker run -d --name mongo -p 27017:27017 mongo:8.0 --replSet rs0 --bind_ip_all
for _ in $(seq 1 60); do
  docker exec mongo mongosh --quiet --eval 'db.runCommand({ ping: 1 }).ok' >/dev/null 2>&1 && break
  sleep 1
done
docker exec mongo mongosh --quiet --eval 'rs.initiate({ _id: "rs0", members: [{ _id: 0, host: "127.0.0.1:27017" }] })'
for _ in $(seq 1 60); do
  [ "$(docker exec mongo mongosh --quiet --eval 'db.hello().isWritablePrimary')" = "true" ] && exit 0
  sleep 1
done
echo "MongoDB didn't become primary" >&2
exit 1
