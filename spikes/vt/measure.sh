#!/bin/bash
# G8 measurement: stripped-size cost of the vt100 adapter and per-pane
# resource cost. Run from spikes/vt.
set -euo pipefail
cd "$(dirname "$0")"

echo "== build: release with vt100 =="
cargo build --release --quiet
echo "== build: release baseline without vt100 =="
cargo build --release --quiet --no-default-features --target-dir target-baseline

strip -x -o /tmp/vt-spike-with target/release/vt-spike
strip -x -o /tmp/vt-spike-without target-baseline/release/vt-spike
with=$(stat -f%z /tmp/vt-spike-with)
without=$(stat -f%z /tmp/vt-spike-without)
echo "stripped_with_vt100_bytes=$with"
echo "stripped_baseline_bytes=$without"
echo "vt100_adapter_delta_bytes=$((with - without))"
echo "vt100_adapter_delta_kib=$(( (with - without) / 1024 ))"

probe() {
  local panes="$1" secs="$2" tag="$3"
  /tmp/vt-spike-with --probe "$panes" "$secs" > "/tmp/vt-probe-$tag.out" &
  local pid=$!
  sleep 3
  local rss fds
  rss=$(ps -o rss= -p "$pid" | tr -d ' ')
  fds=$(lsof -p "$pid" 2>/dev/null | wc -l | tr -d ' ')
  echo "rss_${tag}_kb=$rss"
  echo "fds_${tag}=$fds"
  wait "$pid"
}

RSS_BASE=$(probe 0 6 base | tee /tmp/vt-probe-base.txt | grep -o 'rss_base_kb=[0-9]*' | cut -d= -f2)
RSS_ONE=$(probe 1 8 one | tee /tmp/vt-probe-one.txt | grep -o 'rss_one_kb=[0-9]*' | cut -d= -f2)
RSS_FOUR=$(probe 4 10 four | tee /tmp/vt-probe-four.txt | grep -o 'rss_four_kb=[0-9]*' | cut -d= -f2)
FDS_BASE=$(grep -o 'fds_base=[0-9]*' /tmp/vt-probe-base.txt | cut -d= -f2)
FDS_ONE=$(grep -o 'fds_one=[0-9]*' /tmp/vt-probe-one.txt | cut -d= -f2)
FDS_FOUR=$(grep -o 'fds_four=[0-9]*' /tmp/vt-probe-four.txt | cut -d= -f2)
echo "rss_base_kb=$RSS_BASE rss_one_pane_kb=$RSS_ONE rss_four_panes_kb=$RSS_FOUR"
echo "fds_base=$FDS_BASE fds_one_pane=$FDS_ONE fds_four_panes=$FDS_FOUR"

echo "== full gate suite on the release binary =="
/tmp/vt-spike-with --json evidence.json

python3 - "$with" "$without" "$((with - without))" "$RSS_BASE" "$RSS_ONE" "$RSS_FOUR" "$FDS_BASE" "$FDS_ONE" "$FDS_FOUR" <<'PY'
import json, sys
with_size, without, delta, rb, r1, r4, fb, f1, f4 = sys.argv[1:10]
d = json.load(open("evidence.json"))
d["measurements"] = {
    "stripped_with_vt100_bytes": with_size,
    "stripped_baseline_bytes": without,
    "vt100_adapter_delta_bytes": delta,
    "rss_base_kb": rb,
    "rss_one_pane_kb": r1,
    "rss_four_panes_kb": r4,
    "fds_base": fb,
    "fds_one_pane": f1,
    "fds_four_panes": f4,
}
json.dump(d, open("evidence.json", "w"), indent=2, ensure_ascii=False)
print("evidence.json measurements merged")
PY
