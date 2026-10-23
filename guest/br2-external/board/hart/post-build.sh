#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# The guest has no network, disk, or logging daemons, so drop their init
# scripts; init (see rootfs-overlay/etc/inittab) starts the shell directly.
set -eu
TARGET_DIR="$1"
for s in S01syslogd S02klogd S02sysctl S01seedrng S20seedrng S11modules S40network S50crond; do
    rm -f "$TARGET_DIR/etc/init.d/$s"
done
rm -rf "$TARGET_DIR/lib/modules"
