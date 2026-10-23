# Native boot test

`boot_test.py` is the native boot acceptance test (promise acceptance 4). It:

1. downloads the published guest images (`fw_jump.bin`, `Image`) from the
   release named in `tests/script/commands.json` (`guest_release`) into
   `tests/boot/images/<tag>/`, and checks them against the release's
   `SHA256SUMS`;
2. builds `hart` with `cargo build --release`;
3. runs `hart --images <dir>` under a pseudo-terminal with the shared command
   script runner, `tests/script/run_script.py`: it waits for the shell prompt
   and the "Welcome to hart!" banner, types each command in
   `tests/script/commands.json` (`uname -a`, `cat /proc/cpuinfo`, the awk
   `atan2` check, and `mandelbrot`), and compares the output with the expected
   results, including `tests/script/expected/mandelbrot.txt` byte for byte.

```sh
tests/boot/boot_test.py                      # images from the release, fresh build
tests/boot/boot_test.py --log transcript.txt # also keep the console transcript
tests/boot/boot_test.py --images DIR         # local images (with SHA256SUMS)
tests/boot/boot_test.py -- --timebase insns  # extra arguments for hart
```

It prints one JSON line per step (`boot`, then one per command) and exits 0
only when the prompt is reached and every check passes; 1 means a check failed,
2 a timeout, and 3 a setup error (download, checksum, or build).

Downloads use `gh release download` when `gh` is installed and authenticated,
and plain HTTPS otherwise.
