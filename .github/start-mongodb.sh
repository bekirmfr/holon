#!/usr/bin/env bash
# A MongoDB for CI's tests: a one-node replica set on 127.0.0.1:27017 (a commit is a transaction,
# which needs one), in Docker, ready to write when this returns.
#
# MongoDB 7.0, not 8.0: 8.0 and newer refuse to start on Linux 6.19 and newer (their TCMalloc,
# SERVER-121912, no fixed release yet), the kernel GitHub's ubuntu-24.04 runners have had since
# their 20261025 image. 7.0 keeps the older allocator. Holon asks nothing of MongoDB that 7.0
# lacks (transactions need a replica set, since 4.0).
set -euo pipefail
docker run -d --name mongo -p 27017:27017 mongo:7.0 --replSet rs0 --bind_ip_all
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
