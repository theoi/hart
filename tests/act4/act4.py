#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""ACT4 instrument for hart: generate the official RISC-V architectural tests for the
hart configuration, run every ELF on Sail (self-check) and on hart, and write per-test
pass/fail tables.

Subcommands (see tests/act4/README.md):
  generate     build the hart-rv64gc ELFs (and the upstream sail-RVA20S64 ELFs, for the
               exclusion check) with ACT4 at the pinned commit and Sail 0.16
  exclusions   check that every upstream sail-RVA20S64 test missing from the hart set is
               listed in exclusions.tsv with a reason, and that no listed test is generated
  run          run every hart-rv64gc ELF on one simulator (--sim hart or --sim sail)
  negative     corrupt the expected values in sample ELFs and confirm the simulator fails them
  all          exclusions + run sail + negative sail/hart + run hart, with a summary
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
CONFIG_DIR = HERE / "config" / "hart-rv64gc"
CONFIG_NAME = "hart-rv64gc"
REF_NAME = "sail-RVA20S64"
REF_CONFIG_REL = f"config/sail/{REF_NAME}/test_config.yaml"
ACT4_COMMIT = "46339ac578a5ff9214ef74175344687456659d90"
SAIL_VERSION = "0.16"
# Matches sail.json platform.instructions_per_tick = 1 (see rvmodel_macros.h).
HART_TIMEBASE = "insns:1"
# ELF counts at the pins above (tests/act4/README.md).
EXPECTED_HART_ELFS = 592
EXPECTED_REF_ELFS = 631
EXCLUSIONS = HERE / "exclusions.tsv"
# One sample per suite family for the negative control.
NEGATIVE_SUITES = [
    "I", "M", "Zaamo", "Zalrsc", "F", "D", "Zca", "Zcd", "Zicsr", "Zifencei", "Zicntr", "Zihpm",
    "Zicclsm", "Sm", "S", "U", "Sv", "Svade", "PMPSm", "ExceptionsSm", "InterruptsSm", "InterruptsS",
]

SUMMARY_RE = re.compile(r'RVCP-SUMMARY: TEST (PASSED|FAILED|SIGRUN) - Test File "([^"]*)"')


def default_act4_home() -> Path:
    return Path(os.environ.get("ACT4_HOME", Path.home() / ".cache" / "hart-act4"))


def tool_env(act4_home: Path) -> dict[str, str]:
    env = os.environ.copy()
    extra = [act4_home / "bin", act4_home / "rvgnu" / "bin", act4_home / "sail" / "bin"]
    env["PATH"] = os.pathsep.join([*map(str, extra), env.get("PATH", "")])
    return env


def run_capture(cmd: list[str], **kw) -> str:
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw).stdout.strip()


def elf_dir(work: Path, name: str = CONFIG_NAME) -> Path:
    return work / name / "elfs"


def list_elfs(directory: Path) -> list[str]:
    if not directory.is_dir():
        sys.exit(f"error: no ELF directory at {directory}; run 'generate' first")
    return sorted(str(p.relative_to(directory)) for p in directory.rglob("*.elf"))


# ---------------------------------------------------------------- generate


def cmd_generate(args: argparse.Namespace) -> int:
    act4 = args.act4_home / "riscv-arch-test"
    env = tool_env(args.act4_home)
    head = run_capture(["git", "-C", str(act4), "rev-parse", "HEAD"])
    if head != ACT4_COMMIT:
        sys.exit(f"error: {act4} is at {head}, expected {ACT4_COMMIT} (run tests/act4/setup.sh)")
    sail = run_capture(["sail_riscv_sim", "--version"], env=env)
    if sail != SAIL_VERSION:
        sys.exit(f"error: sail_riscv_sim --version is {sail!r}, expected {SAIL_VERSION}")
    configs = [str(CONFIG_DIR / "test_config.yaml")]
    if not args.no_ref:
        configs.append(str(act4 / REF_CONFIG_REL))
    cmd = ["mise", "exec", "--", "make", f"CONFIG_FILES={' '.join(configs)}", f"WORKDIR={args.work}"]
    if args.jobs:
        cmd.append(f"JOBS={args.jobs}")
    if args.fast:
        cmd.append("FAST=True")
    print("+", " ".join(cmd), flush=True)
    start = time.monotonic()
    subprocess.run(cmd, check=True, cwd=act4, env=env)
    hart = list_elfs(elf_dir(args.work))
    print(f"generated {len(hart)} {CONFIG_NAME} ELFs in {time.monotonic() - start:.0f} s")
    if not args.no_ref:
        print(f"generated {len(list_elfs(elf_dir(args.work, REF_NAME)))} {REF_NAME} ELFs")
    return 0


# ---------------------------------------------------------------- exclusions


def load_exclusions() -> dict[str, str]:
    out: dict[str, str] = {}
    for n, line in enumerate(EXCLUSIONS.read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        test, sep, reason = line.partition("\t")
        if not sep or not reason.strip():
            sys.exit(f"error: {EXCLUSIONS}:{n}: expected '<test>\\t<reason>'")
        out[test.strip()] = reason.strip()
    return out


def check_exclusions(work: Path) -> tuple[bool, str]:
    hart = set(list_elfs(elf_dir(work)))
    ref = set(list_elfs(elf_dir(work, REF_NAME)))
    excl = load_exclusions()
    missing = sorted(ref - hart)
    unlisted = [t for t in missing if t not in excl]
    stale = sorted(t for t in excl if t in hart)
    unknown = sorted(t for t in excl if t not in ref)
    extra = sorted(hart - ref)
    lines = [
        f"# ACT4 exclusion check ({CONFIG_NAME} vs {REF_NAME})",
        "",
        f"- {REF_NAME} ELFs: {len(ref)}",
        f"- {CONFIG_NAME} ELFs: {len(hart)}",
        f"- In {REF_NAME} but not generated for {CONFIG_NAME}: {len(missing)}",
        f"- Of those, listed in exclusions.tsv: {len(missing) - len(unlisted)}",
        f"- Unlisted exclusions: {len(unlisted)}",
        f"- Listed but generated anyway (stale): {len(stale)}",
        f"- Listed but not an upstream test (unknown): {len(unknown)}",
        f"- Generated for {CONFIG_NAME} but not for {REF_NAME}: {len(extra)}",
        "",
        "| Excluded test | Reason |",
        "| --- | --- |",
    ]
    lines += [f"| {t} | {excl.get(t, '**UNLISTED**')} |" for t in missing]
    for title, items in (("Unlisted", unlisted), ("Stale", stale), ("Unknown", unknown), ("Extra", extra)):
        if items:
            lines += ["", f"## {title}", ""] + [f"- {t}" for t in items]
    counts_ok = len(ref) == EXPECTED_REF_ELFS and len(hart) == EXPECTED_HART_ELFS
    if not counts_ok:
        lines += ["", f"## Count mismatch: expected {EXPECTED_REF_ELFS} {REF_NAME} and {EXPECTED_HART_ELFS} {CONFIG_NAME} ELFs"]
    ok = not unlisted and not stale and not unknown and counts_ok
    return ok, "\n".join(lines) + "\n"


def cmd_exclusions(args: argparse.Namespace) -> int:
    ok, report = check_exclusions(args.work)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "exclusions-check.md").write_text(report)
    print(report)
    return 0 if ok else 1


# ---------------------------------------------------------------- running


def sim_command(sim: str, elf: Path, args: argparse.Namespace) -> list[str]:
    if sim == "hart":
        return [str(args.hart), "--elf", str(elf), "--timebase", HART_TIMEBASE, "--timeout", str(args.timeout)]
    return ["sail_riscv_sim", "--config", str(CONFIG_DIR / "sail.json"), str(elf)]


def run_one(sim: str, elf: Path, rel: str, log: Path, args: argparse.Namespace, env: dict[str, str]) -> dict:
    cmd = sim_command(sim, elf, args)
    log.parent.mkdir(parents=True, exist_ok=True)
    start = time.monotonic()
    timed_out = False
    with log.open("w") as f:
        f.write("$ " + " ".join(cmd) + "\n")
        f.flush()
        try:
            proc = subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=f, stderr=subprocess.STDOUT,
                                  env=env, timeout=args.timeout + 30)
            code = proc.returncode
        except subprocess.TimeoutExpired:
            timed_out, code = True, None
    secs = time.monotonic() - start
    text = log.read_text(errors="replace")
    summaries = SUMMARY_RE.findall(text)
    verdicts = [v for v, _ in summaries]
    expected_file = Path(rel).with_suffix(".S").name
    names_ok = bool(summaries) and all(name == expected_file for _, name in summaries)
    if timed_out or (sim == "hart" and code == 124):
        result = "TIMEOUT"
    elif code == 0 and verdicts and all(v == "PASSED" for v in verdicts) and names_ok:
        result = "PASS"
    else:
        result = "FAIL"
    first = next((ln for ln in text.splitlines() if "RVCP-SUMMARY:" in ln), "no RVCP-SUMMARY line")
    return {"test": rel, "suite": Path(rel).parent.name, "result": result, "exit": code,
            "seconds": round(secs, 2), "summary": first.strip(), "log": str(log)}


def run_many(sim: str, items: list[tuple[Path, str, Path]], args: argparse.Namespace) -> list[dict]:
    env = tool_env(args.act4_home)
    results: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs or os.cpu_count() or 1) as pool:
        futs = [pool.submit(run_one, sim, elf, rel, log, args, env) for elf, rel, log in items]
        for n, fut in enumerate(concurrent.futures.as_completed(futs), 1):
            r = fut.result()
            results.append(r)
            if r["result"] != "PASS" or n % 100 == 0 or n == len(futs):
                print(f"[{n}/{len(futs)}] {r['result']:7} {r['test']}", flush=True)
    return sorted(results, key=lambda r: r["test"])


def write_table(results: list[dict], path: Path, title: str, header: list[str]) -> None:
    counts = {k: sum(r["result"] == k for r in results) for k in ("PASS", "FAIL", "TIMEOUT")}
    lines = [f"# {title}", "", *header,
             f"- Total: {len(results)}; PASS {counts['PASS']}; FAIL {counts['FAIL']}; TIMEOUT {counts['TIMEOUT']}",
             "", "| # | Test | Result | Exit | Time (s) | Console summary |", "| ---: | --- | --- | ---: | ---: | --- |"]
    for i, r in enumerate(results, 1):
        summary = r["summary"].replace("|", "\\|")
        lines.append(f"| {i} | {r['test']} | {r['result']} | {r['exit']} | {r['seconds']} | {summary} |")
    path.write_text("\n".join(lines) + "\n")
    tsv = ["test\tsuite\tresult\texit\tseconds"] + [
        f"{r['test']}\t{r['suite']}\t{r['result']}\t{r['exit']}\t{r['seconds']}" for r in results]
    path.with_suffix(".tsv").write_text("\n".join(tsv) + "\n")


def provenance(args: argparse.Namespace) -> dict:
    env = tool_env(args.act4_home)
    act4 = args.act4_home / "riscv-arch-test"

    def git(*a: str, repo: Path = REPO) -> str:
        try:
            return run_capture(["git", "-C", str(repo), *a])
        except (subprocess.CalledProcessError, FileNotFoundError):
            return "unknown"

    info = {
        "recordedAt": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "hart_commit": git("rev-parse", "HEAD"),
        "hart_dirty": git("status", "--porcelain", "--untracked-files=no") != "",
        "act4_config_tree": git("rev-parse", "HEAD:tests/act4"),
        "act4_config_dirty": git("status", "--porcelain", "--", "tests/act4") != "",
        "act4_commit": git("rev-parse", "HEAD", repo=act4),
        "hart_binary": str(args.hart),
    }
    for key, cmd in (("sail_version", ["sail_riscv_sim", "--version"]),
                     ("gcc_version", ["riscv64-unknown-elf-gcc", "--version"])):
        try:
            info[key] = run_capture(cmd, env=env).splitlines()[0]
        except (subprocess.CalledProcessError, FileNotFoundError):
            info[key] = "unknown"
    try:
        info["hart_version"] = run_capture([str(args.hart), "--version"])
    except (subprocess.CalledProcessError, FileNotFoundError, PermissionError):
        info["hart_version"] = "unknown"
    return info


def cmd_run(args: argparse.Namespace, prov: dict | None = None) -> int:
    edir = elf_dir(args.work)
    rels = list_elfs(edir)
    if args.filter:
        rels = [r for r in rels if re.search(args.filter, r)]
    out = args.out
    items = [(edir / r, r, out / "logs" / args.sim / Path(r).with_suffix(".log")) for r in rels]
    print(f"running {len(items)} ELFs on {args.sim}", flush=True)
    start = time.monotonic()
    results = run_many(args.sim, items, args)
    elapsed = time.monotonic() - start
    prov = prov or provenance(args)
    header = [f"- Simulator: {args.sim} ({prov['hart_version'] if args.sim == 'hart' else 'sail_riscv_sim ' + prov['sail_version']})",
              f"- hart commit: {prov['hart_commit']} (dirty: {prov['hart_dirty']}); tests/act4 tree {prov['act4_config_tree']} (dirty: {prov['act4_config_dirty']})",
              f"- ACT4: riscv-arch-test {prov['act4_commit']}; Sail {prov['sail_version']}; {prov['gcc_version']}",
              f"- Recorded: {prov['recordedAt']}; wall time {elapsed:.0f} s; per-test timeout {args.timeout} s",
              f"- Pass criterion: exit status 0 and every RVCP-SUMMARY line reads TEST PASSED for the ELF's own .S file"]
    out.mkdir(parents=True, exist_ok=True)
    write_table(results, out / f"results-{args.sim}.md", f"ACT4 {CONFIG_NAME} on {args.sim}", header)
    bad = [r for r in results if r["result"] != "PASS"]
    print(f"{args.sim}: {len(results) - len(bad)}/{len(results)} PASS; table: {out / f'results-{args.sim}.md'}")
    for r in bad:
        print(f"  {r['result']}: {r['test']} (exit {r['exit']}) log {r['log']}")
    if not args.filter and EXPECTED_HART_ELFS is not None and len(results) != EXPECTED_HART_ELFS:
        print(f"error: {len(results)} ELFs, expected {EXPECTED_HART_ELFS}")
        return 1
    return 1 if bad else 0


# ---------------------------------------------------------------- negative control


def elf_symbol_offsets(data: bytes, names: tuple[str, ...]) -> dict[str, int]:
    """File offsets of the given ELF64 little-endian symbols."""
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        raise ValueError("not an ELF64 little-endian file")
    e_phoff, e_shoff = struct.unpack_from("<QQ", data, 0x20)
    e_phentsize, e_phnum, e_shentsize, e_shnum = struct.unpack_from("<HHHH", data, 0x36)
    shdrs = [struct.unpack_from("<IIQQQQIIQQ", data, e_shoff + i * e_shentsize) for i in range(e_shnum)]
    values: dict[str, int] = {}
    for sh in shdrs:
        if sh[1] != 2:  # SHT_SYMTAB
            continue
        strtab = shdrs[sh[6]]
        for off in range(sh[4], sh[4] + sh[5], sh[9]):
            st_name, _, _, _, st_value, _ = struct.unpack_from("<IBBHQQ", data, off)
            end = data.index(b"\0", strtab[4] + st_name)
            name = data[strtab[4] + st_name:end].decode()
            if name in names:
                values[name] = st_value
    offsets: dict[str, int] = {}
    for name, vaddr in values.items():
        for i in range(e_phnum):
            p_type, _, p_offset, p_vaddr, _, p_filesz, _, _ = struct.unpack_from("<IIQQQQQQ", data, e_phoff + i * e_phentsize)
            if p_type == 1 and p_vaddr <= vaddr < p_vaddr + p_filesz:
                offsets[name] = p_offset + vaddr - p_vaddr
    missing = [n for n in names if n not in offsets]
    if missing:
        raise ValueError(f"symbols not found in a loaded segment: {missing}")
    return offsets


def corrupt_expected(src: Path, dst: Path) -> int:
    """Flip bit 0 of every doubleword of the expected-results region; returns the count."""
    data = bytearray(src.read_bytes())
    offs = elf_symbol_offsets(bytes(data), ("begin_signature", "end_signature"))
    begin, end = offs["begin_signature"], offs["end_signature"]
    count = 0
    for off in range(begin, end - 7, 8):
        data[off] ^= 1
        count += 1
    dst.parent.mkdir(parents=True, exist_ok=True)
    dst.write_bytes(bytes(data))
    return count


def cmd_negative(args: argparse.Namespace) -> int:
    edir = elf_dir(args.work)
    rels = list_elfs(edir)
    chosen = []
    for suite in NEGATIVE_SUITES:
        match = next((r for r in rels if Path(r).parent.name == suite), None)
        if match:
            chosen.append(match)
    out = args.out
    items = []
    for rel in chosen:
        dst = out / "negative" / "elfs" / rel
        corrupt_expected(edir / rel, dst)
        items.append((dst, rel, out / "logs" / f"negative-{args.sim}" / Path(rel).with_suffix(".log")))
    results = run_many(args.sim, items, args)
    detected = [r for r in results if r["result"] == "FAIL" and "TEST FAILED" in r["summary"]]
    lines = [f"# Negative control on {args.sim}", "",
             "Each ELF below is a copy of a generated ELF with bit 0 of every doubleword in its",
             "expected-results region (begin_signature..end_signature) flipped. The simulator must",
             "report RVCP-SUMMARY: TEST FAILED and a nonzero exit status.", "",
             f"- Detected: {len(detected)}/{len(results)}", "",
             "| Test | Result | Exit | Console summary |", "| --- | --- | ---: | --- |"]
    lines += [f"| {r['test']} | {'detected' if r in detected else 'NOT DETECTED'} | {r['exit']} | {r['summary']} |"
              for r in results]
    out.mkdir(parents=True, exist_ok=True)
    (out / f"negative-{args.sim}.md").write_text("\n".join(lines) + "\n")
    print(f"negative control on {args.sim}: {len(detected)}/{len(results)} corrupted ELFs reported TEST FAILED")
    return 0 if len(detected) == len(results) and results else 1


# ---------------------------------------------------------------- all


def cmd_all(args: argparse.Namespace) -> int:
    args.out.mkdir(parents=True, exist_ok=True)
    prov = provenance(args)
    (args.out / "provenance.json").write_text(json.dumps(prov, indent=2) + "\n")
    steps: list[tuple[str, int]] = []
    excl_ok, report = check_exclusions(args.work)
    (args.out / "exclusions-check.md").write_text(report)
    steps.append(("exclusion check", 0 if excl_ok else 1))
    for sim in ("sail", "hart"):
        if sim == "sail" and args.skip_sail:
            continue
        args.sim = sim
        steps.append((f"negative control ({sim})", cmd_negative(args)))
        steps.append((f"run all ELFs ({sim})", cmd_run(args, prov)))
    lines = [f"# ACT4 {CONFIG_NAME} summary", "", "```", json.dumps(prov, indent=2), "```", ""]
    lines += [f"- {name}: {'OK' if rc == 0 else 'FAILED'}" for name, rc in steps]
    (args.out / "summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[-len(steps):]))
    return 0 if all(rc == 0 for _, rc in steps) else 1


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--act4-home", type=Path, default=default_act4_home(),
                   help="toolchain directory from setup.sh (default: $ACT4_HOME or ~/.cache/hart-act4)")
    p.add_argument("--work", type=Path, help="ACT4 work directory (default: <act4-home>/work)")
    p.add_argument("--out", type=Path, default=Path("act4-results"), help="results directory")
    p.add_argument("--hart", type=Path, default=REPO / "target" / "release" / "hart", help="hart binary")
    p.add_argument("--jobs", type=int, default=0, help="parallel jobs (default: CPU count)")
    p.add_argument("--timeout", type=int, default=120, help="per-test timeout in seconds")
    sub = p.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("generate")
    g.add_argument("--no-ref", action="store_true", help=f"skip the {REF_NAME} build (no exclusion check)")
    g.add_argument("--fast", action="store_true", help="skip objdump generation")
    sub.add_parser("exclusions")
    r = sub.add_parser("run")
    r.add_argument("--sim", choices=("hart", "sail"), required=True)
    r.add_argument("--filter", help="regex on the test path (debugging only)")
    n = sub.add_parser("negative")
    n.add_argument("--sim", choices=("hart", "sail"), required=True)
    a = sub.add_parser("all")
    a.add_argument("--skip-sail", action="store_true", help="skip the Sail self-check runs")
    args = p.parse_args()
    args.work = (args.work or args.act4_home / "work").resolve()
    args.out = args.out.resolve()
    args.hart = args.hart.resolve()
    args.filter = getattr(args, "filter", None)
    handlers = {"generate": cmd_generate, "exclusions": cmd_exclusions, "run": cmd_run,
                "negative": cmd_negative, "all": cmd_all}
    return handlers[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
