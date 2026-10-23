#!/bin/sh
# Build the riscv-tests -p ELFs at a pinned commit into tests/riscv-tests/build/.
# Needs riscv64-unknown-elf-gcc (Ubuntu: apt install gcc-riscv64-unknown-elf) and autoconf.
set -eu

COMMIT=3df703c
SUITES="rv64ui rv64um rv64ua rv64uc rv64uf rv64ud rv64mi rv64si"
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=$HERE/src
OUT=$HERE/build
PREFIX=${RISCV_PREFIX:-riscv64-unknown-elf-}

if [ ! -d "$SRC/.git" ]; then
  git clone -q https://github.com/riscv-software-src/riscv-tests.git "$SRC"
fi
cd "$SRC"
git fetch -q origin
git checkout -q "$COMMIT"
git submodule update -q --init --recursive

mkdir -p "$OUT"
cd "$SRC/isa"
# Build only the -p (physical memory) variants.
TARGETS=$(for s in $SUITES; do
  printf 'print:\n\t@echo $(%s_p_tests)\n' "$s" | make -s -f Makefile -f - XLEN=64 RISCV_PREFIX="$PREFIX" print
done)
# shellcheck disable=SC2086
make -s -j"$(nproc)" RISCV_PREFIX="$PREFIX" XLEN=64 $TARGETS >/dev/null
for f in rv64*-p-*; do
  case "$f" in *.dump) continue ;; esac
  cp "$f" "$OUT/"
done
echo "built $(ls "$OUT" | wc -l) ELFs in $OUT"
