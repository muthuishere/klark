# Shared settings for the real-hub load test (E-06). Source me from the repo root.
# The binaries are copied out of target/ so a rebuild cannot pull them from
# under running sessions (refresh with `cp target/release/klark* $K/bin/`).
K=${K:-/private/tmp/claude-501/kload}
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
mkdir -p "$K/bin" "$K/xdg"
for b in klark klark-loadtest; do [ -x "$K/bin/$b" ] || cp "$REPO/target/release/$b" "$K/bin/$b"; done
B=$K/bin/klark
LT=$K/bin/klark-loadtest
export XDG_CONFIG_HOME=$K/xdg
