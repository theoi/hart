#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Builds the hart guest image (OpenSBI + Linux + BusyBox/musl initramfs) from
# pinned upstream sources with Buildroot. No source patches are applied; all
# customization lives in guest/br2-external (defconfig, config fragments,
# rootfs overlay, and the demo package).
#
# Usage: guest/build.sh [--legal-info] [OUTPUT_DIR]
#   OUTPUT_DIR defaults to guest/out. Images and SHA256SUMS land in
#   OUTPUT_DIR/images; --legal-info also produces OUTPUT_DIR/legal-info.tar.
# Environment:
#   BR2_DL_DIR   download cache (default: OUTPUT_DIR/dl)
#   JOBS         parallel make jobs (default: nproc)
set -euo pipefail

BUILDROOT_VERSION=2026.08.1
BUILDROOT_SHA256=34fb7bea8a213ffde39cc02e61703391560f26e0850f03138c722d0b09880f2b
BUILDROOT_URL=https://buildroot.org/downloads/buildroot-${BUILDROOT_VERSION}.tar.xz

legal_info=0
if [ "${1:-}" = "--legal-info" ]; then
    legal_info=1
    shift
fi

here=$(cd "$(dirname "$0")" && pwd)
out=$(mkdir -p "${1:-$here/out}" && cd "${1:-$here/out}" && pwd)
export BR2_DL_DIR=${BR2_DL_DIR:-$out/dl}
jobs=${JOBS:-$(nproc)}
mkdir -p "$BR2_DL_DIR"

tarball=$BR2_DL_DIR/buildroot-${BUILDROOT_VERSION}.tar.xz
if [ ! -f "$tarball" ]; then
    curl -fL --retry 3 -o "$tarball.tmp" "$BUILDROOT_URL"
    mv "$tarball.tmp" "$tarball"
fi
echo "$BUILDROOT_SHA256  $tarball" | sha256sum -c -

src=$out/buildroot-${BUILDROOT_VERSION}
if [ ! -d "$src" ]; then
    tar -C "$out" -xf "$tarball"
fi

# With BR2_REPRODUCIBLE, Buildroot takes SOURCE_DATE_EPOCH from `git log` when
# its source tree is inside any Git checkout (as it is with the default output
# directory), which would tie the images to the hart commit. Pin it to the
# Buildroot release date instead.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(sed -n 's/^BR2_VERSION_EPOCH = //p' "$src/Makefile")}
export SOURCE_DATE_EPOCH

# Fails if the final configuration lacks an option a fragment sets, for example
# because Kconfig dropped it for a missing dependency. An option the fragment
# disables passes when it is unset or absent.
check_fragment() {
    local frag=$1 config=$2 bad=0 line sym
    while IFS= read -r line; do
        case $line in
            CONFIG_*=*)
                if ! grep -qxF -- "$line" "$config"; then
                    echo "not in $config: $line" >&2
                    bad=1
                fi
                ;;
            "# CONFIG_"*" is not set")
                sym=${line#\# }
                sym=${sym%% *}
                if grep -q "^$sym=" "$config"; then
                    echo "not disabled in $config: $sym" >&2
                    bad=1
                fi
                ;;
        esac
    done < "$frag"
    return $bad
}

build=$out/build
make -C "$src" O="$build" BR2_EXTERNAL="$here/br2-external" hart_defconfig
make -C "$build" -j"$jobs"

images=$out/images
rm -rf "$images"
mkdir -p "$images"
bin=$build/images
cp "$bin/fw_jump.bin" "$bin/fw_jump.elf" "$bin/fw_dynamic.bin" "$bin/fw_dynamic.elf" \
   "$bin/Image" "$bin/rootfs.cpio" "$images/"
linux_dir=$(make -C "$build" -s --no-print-directory VARS=LINUX_DIR printvars | sed 's/^LINUX_DIR=//')
busybox_dir=$(make -C "$build" -s --no-print-directory VARS=BUSYBOX_DIR printvars | sed 's/^BUSYBOX_DIR=//')
check_fragment "$here/br2-external/board/hart/linux.fragment" "$linux_dir/.config"
check_fragment "$here/br2-external/board/hart/busybox.fragment" "$busybox_dir/.config"
cp "$linux_dir/.config" "$images/linux.config"
cp "$linux_dir/System.map" "$images/System.map"
(cd "$images" && sha256sum -- * > SHA256SUMS)

if [ "$legal_info" = 1 ]; then
    make -C "$build" legal-info
    # Buildroot does not save its own source, but its skeleton files are in the
    # root filesystem, so add the pinned tarball. Also add the scripts and
    # configuration used to build the images.
    li=$build/legal-info
    cp "$tarball" "$li/"
    tarflags=(--sort=name --owner=0 --group=0 --numeric-owner --mtime=@"$SOURCE_DATE_EPOCH")
    tar -C "$here" "${tarflags[@]}" -cf "$li/hart-guest-build.tar" \
        build.sh README.md br2-external
    (cd "$li" && sha256sum -- "${tarball##*/}" hart-guest-build.tar >> legal-info.sha256)
    cat >> "$li/README" <<EOF
Added by hart's guest/build.sh:
  ${tarball##*/}: the Buildroot source used for this build
  hart-guest-build.tar: guest/build.sh and the br2-external tree (configuration,
  config fragments, root filesystem overlay, and the hart-demos package)
EOF
    tar -C "$build" "${tarflags[@]}" -cf "$out/legal-info.tar" legal-info
fi

cat "$images/SHA256SUMS"
