#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Generate random programs (gen/rvgen.py) for seeds FIRST..LAST, build them,
# run each on Spike and, with --hart, on hart --profile spike, and compare
# the commit logs.
#   run_seeds.sh FIRST LAST OUTDIR [--hart HART_BIN] [--jobs N] [--keep]
# Spike and hart see the same machine: the lockstep flags and device tree in
# lockstep/ (256 MiB, Spike's reset ROM, CLINT, PLIC and UART). Per seed,
# OUTDIR gets <seed>.S and <seed>.elf; the logs (<seed>.spike.log,
# .hart.log, .cmp.txt) are kept only for failing seeds unless --keep is given.
# OUTDIR/summary.txt has one line per seed, "SEED spike=RC lines=N" plus
# " hart=RC cmp=RC" with --hart, and OUTDIR/elf.sha256 the program hashes.
# Exit status: 0 if every seed passed, 1 otherwise.
# Environment: SPIKE (default spike), CC (riscv64-unknown-elf-gcc), NM.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
spike=${SPIKE:-spike}
cc=${CC:-riscv64-unknown-elf-gcc}
nm=${NM:-riscv64-unknown-elf-nm}
dtb=$here/lockstep/spike-sv39.dtb

run_one() {
    s=$1 out=$2 hart=$3 keep=$4
    b=$out/$s
    python3 "$here/gen/rvgen.py" "$s" > "$b.S"
    "$cc" -march=rv64gc -mabi=lp64d -nostdlib -nostartfiles -T "$here/link.ld" -o "$b.elf" "$b.S"
    src=0
    timeout 120 "$spike" --isa=rv64imafdc_zicsr_zifencei_zicntr_zihpm_zicclsm_svade \
        --pmpregions=16 --pmpgranularity=4 --triggers=0 --no-dm -m256 --dtb="$dtb" \
        --log-commits "$b.elf" < /dev/null > /dev/null 2> "$b.spike.log" || src=$?
    line="$s spike=$src lines=$(grep -c '^core' "$b.spike.log" || true)"
    ok=1
    [ "$src" = 0 ] || ok=0
    if [ -n "$hart" ]; then
        hrc=0
        timeout 120 "$hart" --profile spike --bios "$b.elf" --dtb "$dtb" --trace "$b.hart.log" \
            < /dev/null > "$b.hart.out" 2>&1 || hrc=$?
        tohost=0x$("$nm" "$b.elf" | awk '$3 == "tohost" { print $1 }')
        crc=0
        python3 "$here/compare.py" --end-store "$tohost" "$b.spike.log" "$b.hart.log" \
            > "$b.cmp.txt" 2>&1 || crc=$?
        line="$line hart=$hrc cmp=$crc"
        [ "$hrc" = 0 ] && [ "$crc" = 0 ] || ok=0
    fi
    if [ "$ok" = 1 ] && [ "$keep" = 0 ]; then
        rm -f "$b.spike.log" "$b.hart.log" "$b.hart.out" "$b.cmp.txt"
    fi
    echo "$line"
}

if [ "${1:-}" = "--one" ]; then
    shift
    run_one "$@"
    exit 0
fi

if [ $# -lt 3 ]; then
    echo "usage: $0 FIRST LAST OUTDIR [--hart HART_BIN] [--jobs N] [--keep]" >&2
    exit 2
fi
first=$1 last=$2 out=$3
shift 3
hart="" jobs=1 keep=0
while [ $# -gt 0 ]; do
    case $1 in
        --hart) hart=$2; shift 2 ;;
        --jobs) jobs=$2; shift 2 ;;
        --keep) keep=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$out"
out=$(cd "$out" && pwd)
seq "$first" "$last" | xargs -P "$jobs" -I{} sh "$0" --one {} "$out" "$hart" "$keep" \
    | sort -n > "$out/summary.txt"
(cd "$out" && sha256sum -- $(seq -f '%.0f.elf' "$first" "$last")) > "$out/elf.sha256"
total=$(wc -l < "$out/summary.txt")
failed=$(grep -c -v -E ' spike=0 lines=[0-9]+( hart=0 cmp=0)?$' "$out/summary.txt" || true)
echo "$total seeds, $failed failed (details in $out/summary.txt)"
[ "$failed" = 0 ]
