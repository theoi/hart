#!/usr/bin/env bash
# Runs hart's soft-float library against TestFloat vectors. Arguments go to
# `hart-testfloat run`; see README.md. Examples:
#   tests/testfloat/run.sh                      # level 1, every function and rounding mode
#   tests/testfloat/run.sh --level 2            # level 2, core operations, every rounding mode
#   tests/testfloat/run.sh --rm rne f64_mulAdd  # one function, one mode
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ ! -x "${TESTFLOAT_GEN:-$here/build/bin/testfloat_gen}" ]; then
    "$here/build.sh"
fi
cargo build --quiet --release --manifest-path "$here/Cargo.toml"
exec "$here/../../target/release/hart-testfloat" run "$@"
