#!/usr/bin/env bash
# Sample the hub container every ${EVERY:-2} s into a CSV:
#   t, docker mem MiB, docker cpu %, VmRSS KiB, threads, tcp sockets (established / listen / time_wait / other),
#   /data sqlite bytes, assets bytes, net rx/tx bytes (container eth0)
#   sample.sh out.csv
OUT=$1
EVERY=${EVERY:-2}
echo "epoch,mem_mib,cpu_pct,rss_kib,threads,tcp_est,tcp_listen,tcp_tw,tcp_other,db_bytes,assets_bytes,rx_bytes,tx_bytes" > "$OUT"
t0=$(date +%s)
while docker inspect kload-hub >/dev/null 2>&1; do
  s=$(docker stats --no-stream --format '{{.MemUsage}};{{.CPUPerc}}' kload-hub 2>/dev/null) || { sleep "$EVERY"; continue; }
  mem=$(echo "$s" | cut -d';' -f1 | awk '{v=$1; if (v ~ /GiB/) {sub(/GiB/,"",v); v*=1024} else if (v ~ /MiB/) sub(/MiB/,"",v); else if (v ~ /KiB/) {sub(/KiB/,"",v); v/=1024}; print v}')
  cpu=$(echo "$s" | cut -d';' -f2 | tr -d '%')
  inside=$(docker exec kload-hub sh -c '
    rss=$(awk "/VmRSS/{print \$2}" /proc/1/status); th=$(awk "/Threads/{print \$2}" /proc/1/status)
    tcp=$(cat /proc/1/net/tcp /proc/1/net/tcp6 2>/dev/null | awk "NR>1 && \$4 ~ /^[0-9A-F][0-9A-F]\$/ {print \$4}" | sort | uniq -c | awk "{a[\$2]=\$1} END {print (a[\"01\"]+0)\",\"(a[\"0A\"]+0)\",\"(a[\"06\"]+0)\",\"0}")
    db=$(find /data -name "*.db*" -type f -exec stat -c %s {} + 2>/dev/null | awk "{s+=\$1} END {print s+0}")
    as=$(find /data/assets -type f -exec stat -c %s {} + 2>/dev/null | awk "{s+=\$1} END {print s+0}")
    net=$(awk "/eth0/{print \$2\",\"\$10}" /proc/net/dev)
    echo "$rss,$th,$tcp,$db,$as,$net"' 2>/dev/null)
  echo "$(date +%s),$mem,$cpu,$inside" >> "$OUT"
  sleep "$EVERY"
done
