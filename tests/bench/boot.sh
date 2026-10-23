#!/bin/sh
# Boot benchmark: run the guest for N instructions with a fixed timebase and report MIPS.
#   tests/bench/boot.sh <images-dir> [instructions]
set -eu
IMAGES=${1:?usage: boot.sh <images-dir> [instructions]}
N=${2:-300000000}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cargo build -q --release -p hart-cli --manifest-path "$ROOT/Cargo.toml"
LOG=${BENCH_LOG:-/tmp/hart-bench.log}
START=$(date +%s.%N)
"$ROOT/target/release/hart" --images "$IMAGES" --timebase insns:100 --max-insns "$N" </dev/null >"$LOG" 2>&1 || true
END=$(date +%s.%N)
awk -v s="$START" -v e="$END" -v n="$N" 'BEGIN { t = e - s; printf "%d instructions in %.2f s: %.1f MIPS\n", n, t, n / t / 1e6 }'
