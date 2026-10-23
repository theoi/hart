#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Regenerate fixtures/riscv-tests-mix.log.gz: Spike commit logs of riscv-tests
# that together contain every commit-line field (x, f, CSR, load, store, AMO,
# compressed instructions, all privilege modes).
#   make_fixture.sh <spike> <riscv-tests/isa dir>
set -eu
spike=$1
isa=$2
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$here/fixtures"
for t in rv64ui-p-sd rv64ui-p-lb rv64uc-p-rvc rv64um-p-mulh rv64ua-p-amoadd_d rv64ua-p-lrsc \
         rv64uf-p-fcvt rv64ud-p-fmadd rv64ud-p-fcvt_w rv64mi-p-csr rv64si-p-csr rv64si-p-dirty \
         rv64ui-p-ma_data; do
    "$spike" --isa=rv64imafdc_zicsr_zifencei_zicntr_zihpm_zicclsm_svade --pmpregions=16 \
        --log-commits "$isa/$t" 2>&1 >/dev/null
done | gzip -9n > "$here/fixtures/riscv-tests-mix.log.gz"
