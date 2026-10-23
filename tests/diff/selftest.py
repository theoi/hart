#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Mutation self-test for compare.py (check diff-harness-sensitivity).

Compares each fixture log against itself (must match), then against copies with
one injected mutation each, and checks that the comparator reports a divergence
of the right kind at the mutated line. Negative controls (reordered register
entries, interleaved console text) must still match.

    selftest.py [--seed N] [--per-kind N] [LOG ...]

LOG defaults to fixtures/*.log.gz. Prints a JSON summary; exits 0 only if every
mutation is detected at the correct line and every control matches.
"""

import argparse
import glob
import gzip
import io
import json
import os
import random
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import compare  # noqa: E402

HEAD = re.compile(r"^(core\s+\d+: )(\d)( 0x)([0-9a-f]+)( \(0x)([0-9a-f]+)(\))(.*)$")


def flip_hex(h, rng):
    """Flip one random bit of a hex string, keeping its width."""
    v = int(h, 16) ^ (1 << rng.randrange(len(h) * 4))
    return format(v, f"0{len(h)}x")


def split(line):
    m = HEAD.match(line)
    return list(m.groups()) if m else None


def entries(rest):
    """Parse the entry tail into [(kind, tokens)] preserving text."""
    toks = rest.split()
    out, i = [], 0
    while i < len(toks):
        if toks[i] == "mem":
            if i + 2 < len(toks) and toks[i + 2].startswith("0x"):
                out.append(("store", toks[i:i + 3]))
                i += 3
            else:
                out.append(("load", toks[i:i + 2]))
                i += 2
        else:
            out.append((toks[i][0], toks[i:i + 2]))
            i += 2
    return out


def join(parts, ents):
    head = "".join(parts[:7])
    tail = ""
    for kind, toks in ents:
        if kind in ("x", "f"):
            tail += f" {toks[0]:<3} {toks[1]}"
        else:
            tail += " " + " ".join(toks)
    return head + tail


def mutate_field(lines, idx, kind, rng):
    p = split(lines[idx])
    ents = entries(p[7])
    if kind == "pc":
        p[3] = flip_hex(p[3], rng)
    elif kind == "insn":
        p[5] = flip_hex(p[5], rng)
    elif kind == "priv":
        p[1] = str((int(p[1]) + 1 + rng.randrange(2)) % 4)
    elif kind in ("xreg", "freg", "csr"):
        want = {"xreg": "x", "freg": "f", "csr": "c"}[kind]
        cands = [i for i, e in enumerate(ents) if e[0] == want]
        i = rng.choice(cands)
        toks = list(ents[i][1])
        toks[1] = "0x" + flip_hex(toks[1][2:], rng)
        ents[i] = (want, toks)
    elif kind == "xreg-missing":
        cands = [i for i, e in enumerate(ents) if e[0] == "x"]
        del ents[rng.choice(cands)]
    elif kind == "csr-extra":
        ents.insert(0, ("c", ["c832_mscratch", "0x%016x" % rng.getrandbits(64)]))
    elif kind in ("load-addr", "store-addr", "store-value"):
        want = "load" if kind == "load-addr" else "store"
        cands = [i for i, e in enumerate(ents) if e[0] == want]
        i = rng.choice(cands)
        toks = list(ents[i][1])
        j = 2 if kind == "store-value" else 1
        toks[j] = "0x" + flip_hex(toks[j][2:], rng)
        ents[i] = (want, toks)
    else:
        raise ValueError(kind)
    lines[idx] = join(p, ents)


def eligible(line, kind):
    p = split(line)
    if not p:
        return False
    kinds = [e[0] for e in entries(p[7])]
    need = {"xreg": "x", "xreg-missing": "x", "freg": "f", "csr": "c",
            "load-addr": "load", "store-addr": "store", "store-value": "store"}.get(kind)
    return need is None or need in kinds


EXPECT = {
    "pc": ("mismatch", "pc"), "insn": ("mismatch", "insn"), "priv": ("mismatch", "priv"),
    "xreg": ("mismatch", "xreg"), "xreg-missing": ("mismatch", "xreg"),
    "freg": ("mismatch", "freg"), "csr": ("mismatch", "csr"), "csr-extra": ("mismatch", "csr"),
    "load-addr": ("mismatch", "load"), "store-addr": ("mismatch", "store"),
    "store-value": ("mismatch", "store"), "drop": ("missing", None), "extra": ("extra", None),
}


def run(ref_lines, test_lines):
    ref = io.StringIO("\n".join(ref_lines) + "\n")
    test = io.StringIO("\n".join(test_lines) + "\n")
    return compare.compare(ref, test, {"mvip"})


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--per-kind", type=int, default=25)
    ap.add_argument("logs", nargs="*")
    args = ap.parse_args()
    logs = args.logs or sorted(glob.glob(os.path.join(HERE, "fixtures", "*.log.gz")))
    rng = random.Random(args.seed)
    summary = {"seed": args.seed, "logs": [], "kinds": {}, "controls": {}, "failures": []}
    for path in logs:
        op = gzip.open if path.endswith(".gz") else open
        with op(path, "rt", encoding="ascii") as f:
            lines = [ln.rstrip("\n") for ln in f if ln.startswith("core")]
        summary["logs"].append({"path": os.path.relpath(path, HERE), "commits": len(lines)})

        rep = run(lines, lines)
        ok = rep["result"] == "match" and rep["commits"] == len(lines)
        summary["controls"].setdefault("identical", [0, 0])
        summary["controls"]["identical"][0 if ok else 1] += 1
        if not ok:
            summary["failures"].append({"log": path, "control": "identical", "report": rep})

        # Control: reorder register entries on lines with two or more.
        multi = [i for i, ln in enumerate(lines)
                 if sum(e[0] in "xfc" for e in entries(split(ln)[7])) >= 2]
        shuffled = list(lines)
        for i in multi:
            p = split(shuffled[i])
            ents = entries(p[7])
            regs = [e for e in ents if e[0] in "xfc"]
            mem = [e for e in ents if e[0] not in "xfc"]
            shuffled[i] = join(p, regs[::-1] + mem)
        rep = run(lines, shuffled)
        ok = rep["result"] == "match"
        summary["controls"].setdefault("reordered-registers", [0, 0])
        summary["controls"]["reordered-registers"][0 if ok else 1] += 1
        summary["controls"].setdefault("reordered-lines", 0)
        summary["controls"]["reordered-lines"] += len(multi)
        if not ok:
            summary["failures"].append({"log": path, "control": "reordered-registers", "report": rep})

        # Control: console text interleaved between commit lines is ignored.
        noisy = []
        for i, ln in enumerate(lines):
            noisy.append(ln)
            if i % 97 == 0:
                noisy.append("OpenSBI console text that is not a commit line")
        rep = compare.compare(io.StringIO("\n".join(lines) + "\n"),
                              io.StringIO("\n".join(noisy) + "\n"), {"mvip"})
        ok = rep["result"] == "match"
        summary["controls"].setdefault("interleaved-text", [0, 0])
        summary["controls"]["interleaved-text"][0 if ok else 1] += 1

        for kind, (want_kind, want_field) in EXPECT.items():
            cands = [i for i in range(1, len(lines) - 2) if eligible(lines[i], kind)]
            st = summary["kinds"].setdefault(kind, {"detected": 0, "missed": 0})
            for _ in range(args.per_kind if cands else 0):
                idx = rng.choice(cands)
                mut = list(lines)
                if kind == "drop":
                    del mut[idx]
                    want_line = idx + 1  # REF line that TEST lacks
                elif kind == "extra":
                    mut.insert(idx + 1, mut[idx])
                    want_line = idx + 2  # inserted TEST line
                else:
                    mutate_field(mut, idx, kind, rng)
                    want_line = idx + 1
                rep = run(lines, mut)
                got_line = rep.get("ref_line") if kind == "drop" else rep.get("test_line")
                ok = (rep["result"] == "divergence" and rep.get("kind") == want_kind
                      and got_line == want_line
                      and (want_field is None or want_field in rep.get("fields", [])))
                st["detected" if ok else "missed"] += 1
                if not ok:
                    summary["failures"].append({"log": path, "kind": kind, "line": want_line,
                                                "report": rep})
    total = sum(v["detected"] + v["missed"] for v in summary["kinds"].values())
    missed = sum(v["missed"] for v in summary["kinds"].values())
    summary["mutations"] = total
    summary["missed"] = missed
    summary["pass"] = not summary["failures"] and total > 0
    print(json.dumps(summary, indent=1))
    return 0 if summary["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
