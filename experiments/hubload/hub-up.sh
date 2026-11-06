#!/usr/bin/env bash
# Start the real hub image for the load test (E-06).
#   hub-up.sh [extra docker run args, e.g. --memory 512m --cpus 1]
# Machine tokens (test-only, never used anywhere else):
#   kload-fleet  -> fleet entry "load" (klark-loadtest)
#   kload-mac    -> "mac" (the real `klark expose` on this Mac)
set -euo pipefail
. "$(dirname "$0")/env.sh"
PORT=${PORT:-5100}
h() { printf %s "$1" | shasum -a 256 | cut -d' ' -f1; }
docker rm -f kload-hub >/dev/null 2>&1 || true
docker volume create kload-hub-data >/dev/null
docker run -d --name kload-hub -p $PORT:8080 -v kload-hub-data:/data \
  -e KLARK_HUB__AUTH__MODE=MockLdap \
  -e KLARK_HUB__MACHINES__0__NAME=load -e KLARK_HUB__MACHINES__0__FLEET=true \
  -e KLARK_HUB__MACHINES__0__TOKENSHA256=$(h kload-fleet) \
  -e KLARK_HUB__MACHINES__1__NAME=mac \
  -e KLARK_HUB__MACHINES__1__TOKENSHA256=$(h kload-mac) \
  "$@" kload-hub >/dev/null
for _ in $(seq 60); do curl -fsS localhost:$PORT/healthz >/dev/null 2>&1 && break; sleep 0.5; done
echo "hub up on :$PORT"
