#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Compare two commit logs in Spike's --log-commits format.

    compare.py REF TEST

REF is the reference log (Spike) and TEST the log under test (hart). Both may
be files, '-' for stdin, or named pipes, so multi-gigabyte boot logs can be
streamed without storing them. Lines that do not start with "core" (console
output, warnings) are skipped. Each commit line is compared field by field:

    core, privilege mode, pc, instruction bits, register writes (x, f, CSR),
    load addresses (in order), and store address/value/size (in order).

Register writes on one line are compared as a set, because Spike prints them
from an unordered map. Every logged CSR is compared. --drop-csr NAME removes
that CSR from both logs before comparing; nothing is removed by default, and
the option is meant for investigating a divergence, not for check runs.

At the first difference the comparator classifies it: a line missing from
TEST, an extra line in TEST, or a field mismatch. It prints a report and exits
1. Identical logs exit 0. Malformed commit lines exit 2.
"""

import argparse
import itertools
import json
import re
import sys

HEAD = re.compile(r"core\s+(\d+): (\d) 0x([0-9a-f]+) \(0x([0-9a-f]+)\)")
REG = re.compile(r"^(x\d+|f\d+|c\d+_[A-Za-z0-9_.]+)$")
HEX = re.compile(r"^0x[0-9a-f]+$")


class Malformed(Exception):
    pass


class Commit:
    __slots__ = ("lineno", "text", "core", "priv", "pc", "insn", "regs", "loads", "stores")

    def __init__(self, lineno, text, drop):
        self.lineno = lineno
        self.text = text
        m = HEAD.match(text)
        if not m:
            raise Malformed(f"line {lineno}: bad commit header: {text!r}")
        self.core = int(m.group(1))
        self.priv = int(m.group(2))
        self.pc = int(m.group(3), 16)
        self.insn = m.group(4)
        regs, loads, stores = [], [], []
        toks = text[m.end():].split()
        i = 0
        while i < len(toks):
            t = toks[i]
            if REG.match(t):
                if i + 1 >= len(toks) or not HEX.match(toks[i + 1]):
                    raise Malformed(f"line {lineno}: register {t} without value")
                if not (t.startswith("c") and t.split("_", 1)[1] in drop):
                    regs.append((t, int(toks[i + 1], 16)))
                i += 2
            elif t == "mem":
                if i + 1 >= len(toks) or not HEX.match(toks[i + 1]):
                    raise Malformed(f"line {lineno}: mem without address")
                addr = int(toks[i + 1], 16)
                if i + 2 < len(toks) and HEX.match(toks[i + 2]):
                    v = toks[i + 2]
                    stores.append((addr, int(v, 16), (len(v) - 2) // 2))
                    i += 3
                else:
                    if stores:
                        raise Malformed(f"line {lineno}: load after store")
                    loads.append(addr)
                    i += 2
            else:
                raise Malformed(f"line {lineno}: unexpected token {t!r}")
        self.regs = frozenset(regs)
        if len(self.regs) != len(regs):
            raise Malformed(f"line {lineno}: duplicate register entry")
        self.loads = tuple(loads)
        self.stores = tuple(stores)

    def key(self):
        return (self.core, self.priv, self.pc, self.insn, self.regs, self.loads, self.stores)


def commits(stream, drop):
    """Yield (lineno, raw line) for commit lines; parse lazily."""
    for n, line in enumerate(stream, 1):
        if line.startswith("core"):
            yield n, line.rstrip("\n")


def same(a, b, drop):
    if a[1] == b[1]:
        return True
    return Commit(a[0], a[1], drop).key() == Commit(b[0], b[1], drop).key()


def field_diff(ra, rb):
    a, b = ra, rb
    out = []
    for name in ("core", "priv", "pc", "insn"):
        if getattr(a, name) != getattr(b, name):
            out.append(name)
    if a.regs != b.regs:
        kinds = {r[0][0] for r in a.regs ^ b.regs}
        names = {"x": "xreg", "f": "freg", "c": "csr"}
        out.extend(sorted(names[k] for k in kinds))
    if a.loads != b.loads:
        out.append("load")
    if a.stores != b.stores:
        out.append("store")
    return out


def skip_until(it, pc):
    """Drop commit lines until the first one whose pc is `pc`."""
    want = f"0x{pc:016x} "
    return itertools.dropwhile(lambda c: want not in c[1][:40], it)


def stop_after_store(it, addr):
    """End the stream after the first line that stores to `addr` (HTIF tohost)."""
    want = f" mem 0x{addr:016x} 0x"
    for c in it:
        yield c
        if want in c[1]:
            return


def compare(ref, test, drop, limit=None, context=3, start_pc=None, end_store=None, keep_going=0):
    """Return the first divergence report, or a match report.

    With keep_going > 0, up to that many value-only mismatches (same pc and
    instruction) are collected under "earlier" and the walk continues; the
    report then describes the first structural divergence or the end.
    """
    earlier = []
    rep = _compare(ref, test, drop, limit, context, start_pc, end_store, keep_going, earlier)
    if earlier:
        rep["earlier"] = earlier
        if rep["result"] == "match":
            rep["result"] = "divergence"
            rep["kind"] = "value-mismatches"
    return rep


def _compare(ref, test, drop, limit, context, start_pc, end_store, keep_going, earlier):
    ri = commits(ref, drop)
    ti = commits(test, drop)
    if start_pc is not None:
        ri = skip_until(ri, start_pc)
        ti = skip_until(ti, start_pc)
    if end_store is not None:
        ri = stop_after_store(ri, end_store)
        ti = stop_after_store(ti, end_store)
    if limit is not None:
        ri = itertools.islice(ri, limit)
        ti = itertools.islice(ti, limit)
    hist_r, hist_t = [], []
    count = 0
    pend_r, pend_t = [], []

    def nxt(it, pend):
        if pend:
            return pend.pop(0)
        return next(it, None)

    while True:
        a = nxt(ri, pend_r)
        b = nxt(ti, pend_t)
        if a is None and b is None:
            return {"result": "match", "commits": count}
        if a is None or b is None:
            which = "extra" if a is None else "missing"
            at = b if a is None else a
            return {"result": "divergence", "kind": which + "-at-end", "commits": count,
                    "ref_line": a[0] if a else None, "test_line": b[0] if b else None,
                    "ref": a[1] if a else None, "test": b[1] if b else None,
                    "detail": f"{'TEST' if a is None else 'REF'} continues at line {at[0]}"}
        if same(a, b, drop):
            count += 1
            hist_r = (hist_r + [a])[-context:]
            hist_t = (hist_t + [b])[-context:]
            continue
        # Look one line ahead on each side to classify dropped/extra lines.
        a2 = nxt(ri, pend_r)
        if a2 is not None:
            pend_r.insert(0, a2)
        b2 = nxt(ti, pend_t)
        if b2 is not None:
            pend_t.insert(0, b2)
        rep = {"result": "divergence", "commits": count, "ref_line": a[0], "test_line": b[0],
               "ref": a[1], "test": b[1],
               "ref_context": [h[1] for h in hist_r], "test_context": [h[1] for h in hist_t]}
        if a2 is not None and same(a2, b, drop):
            rep["kind"] = "missing"
            rep["detail"] = f"TEST lacks REF line {a[0]}"
        elif b2 is not None and same(a, b2, drop):
            rep["kind"] = "extra"
            rep["detail"] = f"TEST line {b[0]} has no counterpart in REF"
        else:
            rep["kind"] = "mismatch"
            rep["fields"] = field_diff(Commit(a[0], a[1], drop), Commit(b[0], b[1], drop))
            structural = {"core", "priv", "pc", "insn"} & set(rep["fields"])
            if not structural and len(earlier) < keep_going:
                earlier.append({k: rep[k] for k in ("ref_line", "test_line", "ref", "test", "fields")})
                count += 1
                continue
        return rep


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("ref")
    ap.add_argument("test")
    ap.add_argument("--drop-csr", action="append", default=None,
                    help="CSR name to remove from both logs (repeatable; none by default)")
    ap.add_argument("--limit", type=int, help="compare only the first N commit lines of each log")
    ap.add_argument("--start-pc", type=lambda s: int(s, 0),
                    help="skip lines in each log until the first commit at this pc")
    ap.add_argument("--end-store", type=lambda s: int(s, 0),
                    help="stop each log after its first store to this address (e.g. tohost)")
    ap.add_argument("--keep-going", type=int, default=0, metavar="N",
                    help="report up to N value-only mismatches before stopping")
    ap.add_argument("--json", action="store_true", help="print the report as JSON")
    args = ap.parse_args()
    drop = set(args.drop_csr or ())

    def opener(p):
        return sys.stdin if p == "-" else open(p, encoding="ascii", errors="replace", buffering=1 << 20)

    try:
        with opener(args.ref) as r, opener(args.test) as t:
            rep = compare(r, t, drop, args.limit, start_pc=args.start_pc, end_store=args.end_store,
                          keep_going=args.keep_going)
    except Malformed as e:
        print(f"malformed: {e}", file=sys.stderr)
        return 2
    if args.json:
        print(json.dumps(rep))
    elif rep["result"] == "match":
        print(f"match: {rep['commits']} commits")
    else:
        for e in rep.get("earlier", []):
            print(f"value mismatch ({', '.join(e['fields'])}):")
            print(f"  REF  line {e['ref_line']}: {e['ref']}")
            print(f"  TEST line {e['test_line']}: {e['test']}")
        if rep["kind"] == "value-mismatches":
            print(f"DIVERGENCE: {len(rep['earlier'])} value mismatches, no structural divergence")
            return 1
        print(f"DIVERGENCE ({rep['kind']}) after {rep['commits']} matching commits")
        if rep.get("fields"):
            print(f"  fields: {', '.join(rep['fields'])}")
        if rep.get("detail"):
            print(f"  {rep['detail']}")
        for line in rep.get("ref_context", []):
            print(f"    {line}")
        print(f"  REF  line {rep['ref_line']}: {rep['ref']}")
        print(f"  TEST line {rep['test_line']}: {rep['test']}")
    return 0 if rep["result"] == "match" else 1


if __name__ == "__main__":
    sys.exit(main())
