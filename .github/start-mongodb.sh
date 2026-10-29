#!/usr/bin/env bash
# A MongoDB for CI's tests: a one-node replica set on 127.0.0.1:27017 (a commit is a transaction,
# which needs one), in Docker, ready to write when this returns.
#
# The image is pinned by digest: mongo:8.0 as pulled 2026-10-27. The one the tag named on
# 2026-10-29 exited as soon as it started, on every job.
set -euo pipefail
image='mongo:8.0@sha256:8e7d23aa641b0ec6c86418866d5d18d74d5abf89061bc5db7bc4b80929021ef6'
docker run -d --name mongo -p 27017:27017 "$image" --replSet rs0 --bind_ip_all
for _ in $(seq 1 60); do
  docker exec mongo mongosh --quiet --eval 'db.runCommand({ ping: 1 }).ok' >/dev/null 2>&1 && break
  sleep 1
done
if [ "$(docker inspect -f '{{.State.Running}}' mongo)" != "true" ]; then
  echo "MongoDB stopped; its log:" >&2
  docker logs mongo 2>&1 | tail -40 >&2
  exit 1
fi
docker exec mongo mongosh --quiet --eval 'rs.initiate({ _id: "rs0", members: [{ _id: 0, host: "127.0.0.1:27017" }] })'
for _ in $(seq 1 60); do
  [ "$(docker exec mongo mongosh --quiet --eval 'db.hello().isWritablePrimary')" = "true" ] && exit 0
  sleep 1
done
echo "MongoDB didn't become primary" >&2
exit 1
