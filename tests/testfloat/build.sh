#!/usr/bin/env bash
# Builds Berkeley SoftFloat 3 and TestFloat 3 with the RISC-V specialization and
# installs testfloat_gen at tests/testfloat/build/bin/testfloat_gen.
#
# Requires git, make, and a C compiler (gcc). Takes about 30 seconds.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
build="${TESTFLOAT_BUILD_DIR:-$here/build}"
platform="${TESTFLOAT_PLATFORM:-Linux-x86_64-GCC}"
jobs="${JOBS:-$(nproc 2>/dev/null || echo 4)}"

# The official SoftFloat 3e release has no RISC-V specialization; Berkeley's own
# repositories (release 3e plus later fixes) do. Pinned for reproducibility.
softfloat_repo=https://github.com/ucb-bar/berkeley-softfloat-3.git
softfloat_commit=32eb3295a3c814c1551f94ebc29eed1dec3cb3b1
testfloat_repo=https://github.com/ucb-bar/berkeley-testfloat-3.git
testfloat_commit=a9c849f1b0eb0264b626d9686ffae167d996e3be

fetch() {
    local repo="$1" commit="$2" dir="$3"
    if [ -d "$dir/.git" ] && [ "$(git -C "$dir" rev-parse HEAD)" = "$commit" ]; then
        return
    fi
    rm -rf "$dir"
    git init -q "$dir"
    git -C "$dir" fetch -q --depth 1 "$repo" "$commit"
    git -C "$dir" checkout -q FETCH_HEAD
}

mkdir -p "$build/bin"
fetch "$softfloat_repo" "$softfloat_commit" "$build/berkeley-softfloat-3"
fetch "$testfloat_repo" "$testfloat_commit" "$build/berkeley-testfloat-3"

make -s -C "$build/berkeley-softfloat-3/build/$platform" -j"$jobs" SPECIALIZE_TYPE=RISCV softfloat.a
make -s -C "$build/berkeley-testfloat-3/build/$platform" -j"$jobs" \
    SOFTFLOAT_DIR="$build/berkeley-softfloat-3" PLATFORM="$platform" testfloat_gen
cp "$build/berkeley-testfloat-3/build/$platform/testfloat_gen" "$build/bin/testfloat_gen"

grep -q 'init_detectTininess softfloat_tininess_afterRounding' \
    "$build/berkeley-softfloat-3/source/RISCV/specialize.h"
cat > "$build/VERSIONS" <<VERSIONS
berkeley-softfloat-3 $softfloat_commit (SPECIALIZE_TYPE=RISCV)
berkeley-testfloat-3 $testfloat_commit
platform $platform
VERSIONS
echo "built $build/bin/testfloat_gen"
cat "$build/VERSIONS"
