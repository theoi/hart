# Guest image build

`guest/build.sh` builds the guest software that hart boots: OpenSBI firmware, a
Linux kernel, and a BusyBox root filesystem embedded in the kernel as an
initramfs. Everything is built from pinned upstream source releases by a pinned
Buildroot release. No source patches are applied. All customization lives in
`br2-external/` as a defconfig, two config fragments, a root filesystem overlay,
a post-build script, and one small demo package.

The published images are on the
[`guest-v1` release](https://github.com/theoi/hart/releases/tag/guest-v1).

## Building

Requirements: a Linux host with the
[packages Buildroot needs](https://buildroot.org/downloads/manual/manual.html#requirement-mandatory)
(gcc, g++, make, perl, python3, rsync, bc, cpio, unzip, file, wget or curl), about
12 GB of disk, and network access for the source downloads. A build takes about
an hour on 4 cores.

```sh
guest/build.sh                         # output in guest/out
guest/build.sh --legal-info /tmp/hg    # output in /tmp/hg, plus the source and license bundle
BR2_DL_DIR=~/br-dl guest/build.sh      # reuse a download cache
```

The script downloads `buildroot-2026.08.1.tar.xz`, checks its SHA-256, configures
Buildroot with `hart_defconfig`, and builds. Buildroot checks every source it
downloads against a hash (`BR2_DOWNLOAD_FORCE_CHECK_HASHES`). After the build the
script checks that every option set in the two config fragments is in the final
kernel and BusyBox configuration, so the fragments stay an accurate record. Then
it copies the outputs to `OUTPUT_DIR/images` and writes `SHA256SUMS`.

| File | What it is |
|------|------------|
| `fw_jump.bin`, `fw_jump.elf` | OpenSBI `fw_jump` firmware. hart loads it at `0x80000000`. |
| `fw_dynamic.bin`, `fw_dynamic.elf` | OpenSBI `fw_dynamic` firmware, for loaders that pass `struct fw_dynamic_info`. |
| `Image` | Linux kernel with the initramfs built in. Loaded at `0x80200000`. |
| `rootfs.cpio` | The initramfs on its own (already inside `Image`). |
| `linux.config`, `System.map` | Kernel configuration and symbol table, for debugging and review. |
| `SHA256SUMS` | Checksums of the files above. |

`--legal-info` also writes `OUTPUT_DIR/legal-info.tar` (see [Licenses](#licenses)).

The build is reproducible: `BR2_REPRODUCIBLE` fixes timestamps, build paths, user
and host names, so two builds of the same commit produce the same `SHA256SUMS`.
Timestamps, including the kernel build date shown by `uname -a`, come from the
Buildroot release date. The script sets `SOURCE_DATE_EPOCH` to it and sets
`GIT_CEILING_DIRECTORIES` to the output directory: otherwise, when the output
directory is inside a Git checkout (as `guest/out` is), Buildroot takes the
date of that checkout's last commit and appends its commit hash to the
Buildroot version string, which ends up in the compiler and every binary.

## Versions

| Component | Version | Source |
|-----------|---------|--------|
| Buildroot | 2026.08.1 | buildroot.org release tarball |
| Linux | 6.18.55 (6.18 LTS) | kernel.org tarball |
| OpenSBI | 1.8.1 | GitHub release tarball |
| BusyBox | 1.38.0 | busybox.net tarball |
| musl | 1.2.6 | musl.libc.org tarball |
| GCC (cross compiler, builds everything) | 15.3.0 | GNU mirror tarball |
| binutils | 2.45.1 | GNU mirror tarball |

## Booting

The board contract is the same as QEMU's `virt` machine for the parts the guest
uses: RAM at `0x80000000`, an NS16550A UART, CLINT, PLIC, and a syscon
power-off/reboot device, all described by the device tree the machine passes in
`a1`. Any of these boot it to the shell:

```sh
hart --images guest/out/images
qemu-system-riscv64 -machine virt -nographic -bios fw_jump.bin -kernel Image
tests/diff/lockstep/run_spike.sh fw_jump.elf Image
```

The console shows `Welcome to hart!` and then the prompt `~ # `. The guest boots
to the prompt in about 63 million instructions on hart (`hart --stats
--exit-on '~ # ' --timebase insns:100`).

## Configuration choices

### Buildroot (`br2-external/configs/hart_defconfig`)

- **Target RV64GC, LP64D ABI** (`BR2_riscv_g`, `BR2_RISCV_ISA_RVC`,
  `BR2_RISCV_ABI_LP64D`). User space uses exactly the ISA hart implements.
- **Internal musl toolchain** (`BR2_TOOLCHAIN_BUILDROOT_MUSL`). The cross
  compiler and C library are built from source like everything else, and musl
  keeps the root filesystem small. Kernel headers are taken from the same kernel
  version (`BR2_KERNEL_HEADERS_AS_KERNEL`).
- **Hash checks and reproducibility** (`BR2_DOWNLOAD_FORCE_CHECK_HASHES`,
  `BR2_REPRODUCIBLE`). The custom kernel version needs a hash file, and Buildroot
  only reads extra hash files from a patch directory, so `BR2_GLOBAL_PATCH_DIR`
  points at `board/hart/hashes`. That directory holds only
  `linux/linux.hash`, with the kernel.org checksum; it contains no patches.
- **Linux 6.18.55** (`BR2_LINUX_KERNEL_CUSTOM_VERSION_VALUE`), configured as the
  upstream RISC-V `defconfig` plus `board/hart/linux.fragment`.
- **BusyBox** with Buildroot's default configuration plus
  `board/hart/busybox.fragment`.
- **Init and console.** BusyBox init with `rootfs-overlay/etc/inittab`, which
  runs a root login shell directly on `ttyS0` instead of a getty
  (`BR2_TARGET_GENERIC_GETTY` off). Using the tty device rather than
  `/dev/console` gives the shell a controlling terminal, so job control and
  Ctrl-C work. `rootfs-overlay/etc/profile.d/hart.sh` sets `HOME=/root`, the
  prompt (`PS1='\w \$ '`, which shows `~ # ` in the home directory), and prints
  the banner. The hostname is `hart`, and `/etc/issue` also carries the banner.
- **No network setup** (`BR2_SYSTEM_DHCP` empty, `BR2_PACKAGE_IFUPDOWN_SCRIPTS`
  off). The board has no network device.
- **Root filesystem as an initramfs** (`BR2_TARGET_ROOTFS_CPIO`,
  `BR2_TARGET_ROOTFS_INITRAMFS`). The cpio archive is built into `Image`, so the
  machine only needs the firmware and the kernel. The board has no disk.
- **OpenSBI** (`BR2_TARGET_OPENSBI_LATEST_VERSION`, which is 1.8.1 in this
  Buildroot release), `generic` platform, installing both `fw_jump` and
  `fw_dynamic`. `FW_JUMP_FDT_ADDR=0x88000000` makes `fw_jump` copy the device
  tree 128 MiB into RAM before starting Linux. The default (`0x82200000`) is
  only 32 MiB above the kernel's load address, close to the end of the kernel
  with its built-in initramfs.

### Root filesystem

- `board/hart/post-build.sh` removes the init scripts for daemons the board
  cannot use (syslogd, klogd, sysctl, seedrng, module loading, network, crond)
  and the empty `/lib/modules`.
- `package/hart-demos` builds `/usr/bin/mandelbrot`, an ASCII Mandelbrot set
  computed in double precision. It is compiled with `-ffp-contract=off` so that
  every multiply and add is rounded separately; its output is then the same on
  any IEEE 754 machine, which makes it a quick check of the FPU. Its source is
  in `package/hart-demos/src` (MIT OR Apache-2.0).
- `busybox.fragment` enables `CONFIG_FEATURE_AWK_LIBM`, so `awk` has `atan2`,
  `sin`, `exp`, and the other math functions.

### Kernel (`board/hart/linux.fragment`)

The fragment trims the upstream defconfig to what a single-hart virt-style board
needs, mostly to cut boot time. Each group in the file has a comment; in short:

- `EXPERT` and `NONPORTABLE` make the options below switchable. Without
  `EXPERT`, options such as `BLOCK` and `VT` cannot be disabled. Without
  `NONPORTABLE`, the RISC-V `PORTABLE` option selects `EFI`, and
  `RISCV_EFFICIENT_UNALIGNED_ACCESS` is unavailable.
- Off: modules, tracing, kprobes, profiling, debug info, debugfs, EFI, ACPI,
  32-bit compat, the vector extension (hart has none), KVM, SoC platforms other
  than `ARCH_VIRT`, suspend, hibernation, cpuidle, cpufreq, and all subsystems
  with no device on the board (network, PCI, block, USB, graphics, VT, sound,
  MMC, MTD, input, HID, virtio, hwrng, I2C, SPI, thermal, watchdog, media, IIO,
  PWM, DMA, mailbox, IOMMU, RTC, other UART drivers, legacy PTYs).
- `RISCV_EFFICIENT_UNALIGNED_ACCESS`: hart handles misaligned loads and stores
  in hardware, so the kernel skips its boot-time misaligned-access timing probe.
- `CRYPTO` off: nothing in the guest uses the kernel crypto API, and its
  jitter-entropy self-test alone costs about 126 million instructions at boot.
- Off: cgroups, namespaces, BPF syscall, security modules, audit, keys, swap.
- On: the 8250 UART and its console, the SBI early console, syscon power-off
  and reboot, tmpfs, devtmpfs, and an uncompressed built-in initramfs
  (decompressing it would cost boot time for a 1.7 MB archive).

## Licenses

`--legal-info` runs Buildroot's `legal-info` target and packs its output into
`legal-info.tar`. The bundle contains:

- `manifest.csv` and `host-manifest.csv`: every package with its version,
  license, license files, and source archive;
- `sources/` and `host-sources/`: the exact source archives of every package
  used for the target and the build tools (Linux, OpenSBI, BusyBox, musl, GCC,
  and the rest), and `licenses/` and `host-licenses/` with their license texts;
- `buildroot-2026.08.1.tar.xz`: Buildroot itself (its skeleton files are part of
  the root filesystem);
- `hart-guest-build.tar`: this directory's scripts and configuration;
- `buildroot.config`: the full Buildroot configuration.

| Component in the images | License |
|-------------------------|---------|
| Linux | GPL-2.0 (with Linux-syscall-note) |
| OpenSBI | BSD-2-Clause |
| BusyBox | GPL-2.0 (bzip2 code: bzip2-1.0.6) |
| musl | MIT |
| GCC runtime (libgcc) | GPL-3.0 with GCC Runtime Library Exception |
| Buildroot skeleton files | GPL-2.0-or-later |
| hart-demos | MIT OR Apache-2.0 |

hart's own license does not cover the guest software.

## Publishing a release

```sh
guest/build.sh --legal-info /tmp/hg
gh release create guest-vN -R theoi/hart --title guest-vN --notes-file NOTES \
    /tmp/hg/images/* /tmp/hg/legal-info.tar
```

The boot tests (`tests/boot`, `tests/web`) download the release named by
`guest_release` in `tests/script/commands.json`, and the expected outputs in
`tests/script/expected` must match what the new images print.
