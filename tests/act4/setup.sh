#!/usr/bin/env bash
# Installs the pinned ACT4 toolchain used by tests/act4 into $ACT4_HOME
# (default: ~/.cache/hart-act4):
#   riscv-arch-test (act4 branch) at the pinned commit,
#   riscv-collab prebuilt riscv64-unknown-elf GCC, Sail RISC-V model 0.16 (prebuilt),
#   mise, and through mise: uv, Ruby, Bundler, and the UDB gems ACT4 needs.
# Linux x86_64 only. Safe to re-run; finished steps are skipped.
set -euo pipefail

ACT4_HOME=${ACT4_HOME:-$HOME/.cache/hart-act4}
ACT4_REPO=https://github.com/riscv-non-isa/riscv-arch-test.git
ACT4_COMMIT=46339ac578a5ff9214ef74175344687456659d90
GCC_URL=https://github.com/riscv-collab/riscv-gnu-toolchain/releases/download/2026.10.21/riscv64-elf-ubuntu-24.04-gcc.tar.xz
GCC_SHA256=c7d08d4cb38ef0c588d41d6077f192930d7427d56f9555a1547e53d0efccbfd2
SAIL_URL=https://github.com/riscv/sail-riscv/releases/download/0.16/sail-riscv-Linux-x86_64.tar.gz
SAIL_SHA256=f9bb8a32b8f3eddbdf69b3a29d96dff4ba1e77f45877165e0478418367da6bee
MISE_URL=https://github.com/jdx/mise/releases/download/v2026.10.17/mise-v2026.10.17-linux-x64
MISE_SHA256=fe6dd17e68e09682b31d2e7bb7bb755d22636ac1af5d94621e300f2ca9e9fe8a

mkdir -p "$ACT4_HOME/dl" "$ACT4_HOME/bin"
cd "$ACT4_HOME"

fetch() { # url sha256 output
  if [ ! -f "$3" ] || ! echo "$2  $3" | sha256sum -c --quiet - 2>/dev/null; then
    curl -sSfL -o "$3.part" "$1"
    echo "$2  $3.part" | sha256sum -c --quiet -
    mv "$3.part" "$3"
  fi
}

if [ ! -x rvgnu/bin/riscv64-unknown-elf-gcc ]; then
  echo "Installing riscv64-unknown-elf GCC..."
  fetch "$GCC_URL" "$GCC_SHA256" dl/gcc.tar.xz
  mkdir -p rvgnu && tar -xJf dl/gcc.tar.xz -C rvgnu --strip-components=1
fi

if [ ! -x sail/bin/sail_riscv_sim ]; then
  echo "Installing Sail 0.16..."
  fetch "$SAIL_URL" "$SAIL_SHA256" dl/sail.tar.gz
  mkdir -p sail && tar -xzf dl/sail.tar.gz -C sail --strip-components=1
fi

if [ ! -x bin/mise ]; then
  echo "Installing mise..."
  fetch "$MISE_URL" "$MISE_SHA256" bin/mise
  chmod +x bin/mise
fi

if [ ! -d riscv-arch-test/.git ]; then
  echo "Cloning riscv-arch-test..."
  git clone -q "$ACT4_REPO" riscv-arch-test
fi
git -C riscv-arch-test fetch -q origin act4
git -C riscv-arch-test checkout -q "$ACT4_COMMIT"

export PATH="$ACT4_HOME/bin:$ACT4_HOME/rvgnu/bin:$ACT4_HOME/sail/bin:$PATH"
cd riscv-arch-test
mise trust -q .mise.toml
mise install

echo
echo "ACT4 toolchain ready in $ACT4_HOME"
echo "  riscv-arch-test $(git rev-parse --short HEAD)"
echo "  $(riscv64-unknown-elf-gcc --version | head -1)"
echo "  sail_riscv_sim $(sail_riscv_sim --version)"
