#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Native boot acceptance test: boot hart to the shell and run the command script.

Fetches the published guest images (verified against the release's
SHA256SUMS), builds the `hart` binary in release mode, and runs the shared
command script (tests/script/commands.json) against `hart` under a
pseudo-terminal using tests/script/run_script.py. Exit status: 0 when every
check passes, 1 when a check fails, 2 when the prompt never appears or a
command times out, 3 for setup errors (download, checksum, build).
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
SCRIPT_DIR = os.path.join(ROOT, "tests", "script")
REPO = "theoi/hart"
ASSETS = ["fw_jump.bin", "Image"]


def log(msg):
    print(f"boot_test: {msg}", file=sys.stderr, flush=True)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def parse_sums(path):
    sums = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            parts = line.split()
            if len(parts) == 2:
                sums[parts[1].lstrip("*")] = parts[0].lower()
    return sums


def download(tag, names, dest):
    os.makedirs(dest, exist_ok=True)
    if shutil.which("gh"):
        cmd = ["gh", "release", "download", tag, "-R", REPO, "-D", dest, "--clobber"]
        for name in names:
            cmd += ["-p", name]
        if subprocess.run(cmd).returncode == 0:
            return
        log("gh release download failed; trying a direct download")
    for name in names:
        url = f"https://github.com/{REPO}/releases/download/{tag}/{name}"
        log(f"fetching {url}")
        with urllib.request.urlopen(url) as r, open(os.path.join(dest, name), "wb") as f:
            shutil.copyfileobj(r, f)


def fetch_images(tag, dest):
    """Ensure dest holds the release's boot images with matching checksums."""
    sums_path = os.path.join(dest, "SHA256SUMS")
    if not os.path.exists(sums_path):
        download(tag, ["SHA256SUMS"], dest)
    sums = parse_sums(sums_path)
    for name in ASSETS:
        if name not in sums:
            raise RuntimeError(f"{name} is not listed in {tag}'s SHA256SUMS")
    stale = [n for n in ASSETS if not os.path.exists(os.path.join(dest, n))
             or sha256(os.path.join(dest, n)) != sums[n]]
    if stale:
        download(tag, stale, dest)
    for name in ASSETS:
        got = sha256(os.path.join(dest, name))
        if got != sums[name]:
            raise RuntimeError(f"{name}: sha256 {got} does not match SHA256SUMS ({sums[name]})")
        log(f"{name} sha256 {got} OK")


def main():
    with open(os.path.join(SCRIPT_DIR, "commands.json"), encoding="utf-8") as f:
        default_tag = json.load(f)["guest_release"]
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--release", default=default_tag,
                    help=f"guest image release tag on {REPO} (default: {default_tag}, from commands.json)")
    ap.add_argument("--images", help="use images from this directory instead of downloading "
                    "(must contain fw_jump.bin, Image, and SHA256SUMS)")
    ap.add_argument("--hart", help="hart binary to test (default: build target/release/hart)")
    ap.add_argument("--script", default=os.path.join(SCRIPT_DIR, "commands.json"),
                    help="command script (default: tests/script/commands.json)")
    ap.add_argument("--log", help="write the raw console transcript here")
    ap.add_argument("--save-outputs", help="directory to write each command's output to")
    ap.add_argument("--boot-timeout", type=float, help="override boot_timeout_seconds")
    ap.add_argument("hart_args", nargs="*", help="extra arguments for hart (after --)")
    args = ap.parse_args()

    images = args.images or os.path.join(HERE, "images", args.release)
    try:
        if args.images:
            sums = parse_sums(os.path.join(images, "SHA256SUMS"))
            for name in ASSETS:
                if sha256(os.path.join(images, name)) != sums.get(name):
                    raise RuntimeError(f"{images}/{name} does not match SHA256SUMS")
        else:
            fetch_images(args.release, images)
        hart = args.hart
        if not hart:
            log("building hart (cargo build --release -p hart-cli)")
            subprocess.run(["cargo", "build", "--release", "--locked", "-p", "hart-cli"],
                           cwd=ROOT, check=True)
            hart = os.path.join(ROOT, "target", "release", "hart")
    except (RuntimeError, OSError, subprocess.CalledProcessError) as e:
        log(f"setup failed: {e}")
        return 3

    runner = [sys.executable, os.path.join(SCRIPT_DIR, "run_script.py"), "--script", args.script]
    if args.log:
        runner += ["--log", args.log]
    if args.save_outputs:
        runner += ["--save-outputs", args.save_outputs]
    if args.boot_timeout:
        runner += ["--boot-timeout", str(args.boot_timeout)]
    runner += ["--", hart, "--images", images] + args.hart_args
    log("running " + " ".join(runner))
    return subprocess.run(runner).returncode


if __name__ == "__main__":
    sys.exit(main())
