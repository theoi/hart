#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Boot the guest on Spike in the lockstep reference configuration.
#   run_spike.sh FW_JUMP_ELF IMAGE [extra spike args, e.g. --log-commits --log=FILE]
# The same spike-sv39.dtb must be given to hart: hart --profile spike --dtb spike-sv39.dtb
set -eu
here=$(cd "$(dirname "$0")" && pwd)
fw=$1
kernel=$2
shift 2
exec "${SPIKE:-spike}" --isa=rv64imafdc_zicsr_zifencei_zicntr_zihpm_zicclsm_svade \
    --pmpregions=16 --pmpgranularity=4 --triggers=0 --no-dm -m256 \
    --dtb="$here/spike-sv39.dtb" --kernel="$kernel" "$@" "$fw"
