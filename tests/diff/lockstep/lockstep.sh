#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Boot the guest on Spike and on hart --profile spike at the same time and
# compare their commit logs as they stream (nothing large is written to disk).
#   lockstep.sh FW_JUMP_ELF IMAGE LIMIT [OUTDIR]
# LIMIT is the number of commit lines to compare. OUTDIR (default
# target/lockstep) receives compare.txt and both consoles. Environment: SPIKE
# (default: spike on PATH), HART (a prebuilt hart binary; by default the script
# runs cargo build --release and uses target/release/hart), COMPARE_ARGS (extra
# compare.py flags, e.g. "--keep-going 1000").
# Exit status is compare.py's: 0 match, 1 divergence, 2 malformed log.
set -u
if [ $# -lt 3 ]; then
    echo "usage: $0 FW_JUMP_ELF IMAGE LIMIT [OUTDIR]" >&2
    exit 2
fi
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
fw=$1
kernel=$2
limit=$3
out=${4:-$root/target/lockstep}
if [ -z "${HART:-}" ]; then
    (cd "$root" && cargo build --release --locked -p hart-cli) || exit 2
    HART=$root/target/release/hart
fi
for f in "$fw" "$kernel" "$HART"; do
    if [ ! -f "$f" ]; then
        echo "lockstep: missing $f" >&2
        exit 2
    fi
done
mkdir -p "$out"
rm -f "$out/spike.pipe" "$out/hart.pipe"
mkfifo "$out/spike.pipe" "$out/hart.pipe"

"$here/run_spike.sh" "$fw" "$kernel" --log-commits --log="$out/spike.pipe" \
    < /dev/null > "$out/spike-console.log" 2>&1 &
spike_pid=$!
# hart retires a few more instructions than compared so its log never ends first.
"$HART" --profile spike --bios "$fw" --kernel "$kernel" \
    --dtb "$here/spike-sv39.dtb" --trace "$out/hart.pipe" --max-insns $((limit + 1000)) \
    < /dev/null > "$out/hart-console.log" 2>&1 &
hart_pid=$!
trap 'kill $spike_pid $hart_pid 2>/dev/null; rm -f "$out/spike.pipe" "$out/hart.pipe"; exit 130' INT TERM

# shellcheck disable=SC2086 # COMPARE_ARGS is a list of flags
python3 "$here/../compare.py" --limit "$limit" ${COMPARE_ARGS:-} "$out/spike.pipe" "$out/hart.pipe" \
    > "$out/compare.txt" 2>&1
status=$?
kill $spike_pid $hart_pid 2>/dev/null
wait 2>/dev/null
rm -f "$out/spike.pipe" "$out/hart.pipe"

cat "$out/compare.txt"
# Spike is stopped at an arbitrary point after hart, so its console may run longer.
s=$(wc -c < "$out/spike-console.log")
h=$(wc -c < "$out/hart-console.log")
n=$((s < h ? s : h))
if cmp -s -n "$n" "$out/spike-console.log" "$out/hart-console.log"; then
    echo "consoles agree ($h bytes from hart, $s from Spike)"
else
    echo "consoles differ: compare $out/spike-console.log and $out/hart-console.log"
fi
exit $status
