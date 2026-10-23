#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Run the hart command script (commands.json) against a console program.

The program is started under a pseudo-terminal. The script waits for the shell
prompt, types each command, and checks the output as tests/script/README.md
describes. Exit status: 0 when every check passes, 1 when a check fails, and 2
when the prompt never appears or a command times out.
"""

import argparse
import fcntl
import json
import os
import pty
import re
import select
import signal
import struct
import sys
import termios
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_]")


def normalize(text):
    text = ANSI.sub("", text)
    return text.replace("\r\n", "\n").replace("\r", "")


def check(spec, output, base):
    kind = spec["type"]
    if kind == "exact":
        return output == spec["text"]
    if kind == "exact_file":
        with open(os.path.join(base, spec["path"]), encoding="utf-8") as f:
            want = f.read()
        if want.endswith("\n"):
            want = want[:-1]
        return output == want
    if kind == "contains":
        return spec["text"] in output
    if kind == "regex":
        flags = re.MULTILINE if "m" in spec.get("flags", "") else 0
        return re.search(spec["pattern"], output, flags) is not None
    raise ValueError(f"unknown check type {kind!r}")


class Console:
    def __init__(self, argv, cols, rows, log):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execvp(argv[0], argv)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.raw = b""
        self.log = log

    def text(self):
        return normalize(self.raw.decode("utf-8", errors="replace"))

    def send(self, s):
        os.write(self.fd, s.encode())

    def wait_for(self, regex, start, timeout):
        """Wait until regex matches the normalized text after offset start."""
        deadline = time.monotonic() + timeout
        while True:
            text = self.text()
            m = regex.search(text, start)
            if m:
                return text, m
            left = deadline - time.monotonic()
            if left <= 0:
                return text, None
            r, _, _ = select.select([self.fd], [], [], min(left, 0.5))
            if r:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    data = b""
                if not data:
                    return self.text(), None
                self.raw += data
                if self.log:
                    self.log.write(data)
                    self.log.flush()

    def close(self):
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.kill(self.pid, sig)
            except ProcessLookupError:
                break
            for _ in range(20):
                if os.waitpid(self.pid, os.WNOHANG) != (0, 0):
                    return
                time.sleep(0.1)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--script", default=os.path.join(HERE, "commands.json"))
    ap.add_argument("--log", help="write the raw console transcript here")
    ap.add_argument("--boot-timeout", type=float, help="override boot_timeout_seconds")
    ap.add_argument("--command-timeout", type=float, help="override command_timeout_seconds")
    ap.add_argument("--save-outputs", help="directory to write each command's output to")
    ap.add_argument("argv", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    argv = args.argv[1:] if args.argv[:1] == ["--"] else args.argv
    if not argv:
        ap.error("missing console command after --")

    with open(args.script, encoding="utf-8") as f:
        script = json.load(f)
    base = os.path.dirname(os.path.abspath(args.script))
    prompt = re.compile(script["prompt"]["regex"] + r"\Z")
    boot_timeout = args.boot_timeout or script["boot_timeout_seconds"]
    cmd_timeout = args.command_timeout or script["command_timeout_seconds"]
    term = script["terminal"]

    log = open(args.log, "wb") if args.log else None
    con = Console(argv, term["cols"], term["rows"], log)
    status = 0
    try:
        t0 = time.monotonic()
        text, m = con.wait_for(prompt, 0, boot_timeout)
        if not m:
            print(json.dumps({"id": "boot", "pass": False, "error": "prompt not reached"}))
            return 2
        boot = time.monotonic() - t0
        banner_ok = script["banner"] + "\n" in text[: m.start() + 1]
        print(json.dumps({"id": "boot", "pass": banner_ok, "seconds": round(boot, 3),
                          "banner": banner_ok}))
        if not banner_ok:
            status = 1
        for cmd in script["commands"]:
            start = len(text)
            con.send(cmd["input"] + script["enter"])
            text, m = con.wait_for(prompt, start, cmd_timeout)
            if not m:
                print(json.dumps({"id": cmd["id"], "pass": False, "error": "timeout"}))
                return 2
            segment = text[start:m.start()]
            nl = segment.find("\n")
            output = segment[nl + 1:] if nl >= 0 else ""
            if output.endswith("\n"):
                output = output[:-1]
            results = [check(c, output, base) for c in cmd["checks"]]
            ok = all(results)
            print(json.dumps({"id": cmd["id"], "pass": ok, "checks": results,
                              "output": output if not ok else None}))
            if args.save_outputs:
                os.makedirs(args.save_outputs, exist_ok=True)
                with open(os.path.join(args.save_outputs, cmd["id"] + ".txt"), "w",
                          encoding="utf-8") as f:
                    f.write(output + "\n")
            if not ok:
                status = 1
        return status
    finally:
        con.close()
        if log:
            log.close()


if __name__ == "__main__":
    sys.exit(main())
