# Command script

`commands.json` is the fixed command script that the acceptance tests type into
the guest shell after boot (promise acceptance 4, native, and 5, browser). Both
test suites read this file, so the commands and expected output are defined in
one place. `expected/` holds expected outputs that are too long to inline.

## Protocol

1. Start the machine with a terminal of `terminal.cols` x `terminal.rows`.
2. Wait up to `boot_timeout_seconds` until the **normalized** console text
   matches `prompt.regex`. The prompt must be at the very end of the text, with
   nothing after it. `banner` appears on the line before the first prompt.
3. For each entry in `commands`, in order:
   - Note the current end of the console text, then send `input` followed by
     `enter` (a carriage return, as a terminal sends for the Enter key).
   - Wait up to `command_timeout_seconds` until the normalized text after that
     point matches `prompt.regex` again.
   - The command's **output** is the normalized text after the first `\n`
     following that point (the end of the echoed command line), up to the start
     of the prompt match. One trailing `\n` is removed.
   - Apply every check to the output. All must pass.

## Normalization

Apply in this order to the raw console bytes, decoded as UTF-8:

1. Remove ANSI escape sequences: CSI sequences (`ESC [` parameters, then a final
   byte in `@`–`~`) and two-byte `ESC` sequences.
2. Replace `\r\n` with `\n`, then remove any remaining `\r`.

## Checks

| type         | passes when                                                         |
|--------------|---------------------------------------------------------------------|
| `exact`      | output equals `text`                                                |
| `exact_file` | output equals the file at `path` (relative to this directory), minus its final newline |
| `contains`   | output contains `text`                                              |
| `regex`      | `pattern` matches somewhere in the output; `flags` may contain `m` (multiline) |

Patterns use the subset of syntax shared by Python `re` and JavaScript
`RegExp`: no inline flags, lookbehind, or named groups.

## Expected outputs

- `expected/mandelbrot.txt` is the output of `/usr/bin/mandelbrot` from the
  published guest image run on QEMU (bare RV64GC CPU) and on Spike. Both runs must
  agree byte for byte with each other. The program uses only IEEE 754 double
  arithmetic built with `-ffp-contract=off`, so a host build of
  `guest/br2-external/package/hart-demos/src/mandelbrot.c` produces the same
  output.

## Reference runner

`run_script.py` drives any console program through a pseudo-terminal and
applies this protocol:

```sh
tests/script/run_script.py --log transcript.txt -- \
    qemu-system-riscv64 -machine virt -nographic -bios fw_jump.bin -kernel Image
```

It prints one JSON result per command and exits non-zero if the prompt is not
reached or any check fails.
