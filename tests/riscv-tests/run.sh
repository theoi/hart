#!/bin/sh
# Run riscv-tests -p ELFs on hart. Default: the integer and privileged suites.
#   tests/riscv-tests/run.sh                 # rv64ui um ua uc mi si
#   tests/riscv-tests/run.sh rv64uf rv64ud   # selected suites
#   tests/riscv-tests/run.sh path/to/x.elf   # single ELFs (add --trace for a commit log on stderr)
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
OUT=$HERE/build

cargo build -q --release -p hart-core --example riscv-tests --manifest-path "$ROOT/Cargo.toml"
BIN=$ROOT/target/release/examples/riscv-tests

if [ $# -eq 0 ]; then
  set -- rv64ui rv64um rv64ua rv64uc rv64mi rv64si
fi
ARGS=""
for a in "$@"; do
  case "$a" in
    rv64*) ARGS="$ARGS $(ls "$OUT"/"$a"-p-* | tr '\n' ' ')" ;;
    *) ARGS="$ARGS $a" ;;
  esac
done
# shellcheck disable=SC2086
exec "$BIN" $ARGS
