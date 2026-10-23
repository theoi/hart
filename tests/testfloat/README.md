# TestFloat conformance harness

Checks hart's soft-float library (`crates/hart-core/src/softfloat`) against
[Berkeley TestFloat 3](http://www.jhauser.us/arithmetic/TestFloat.html), bit for bit,
including the five exception flags.

TestFloat and SoftFloat are used only as references. Nothing from them is linked
into hart.

## Quick start

```sh
tests/testfloat/build.sh          # once: fetch, build, install testfloat_gen (~30 s)
tests/testfloat/run.sh            # level 1: every function x every rounding mode (~1 min)
tests/testfloat/run.sh --level 2  # level 2: core operations x every rounding mode
```

`run.sh` builds `hart-testfloat` in release mode, then runs it. Each run prints one row per
function and rounding mode, plus a final `TOTAL` line:

```text
function     rm        vectors mismatches
f64_mulAdd   rne       6133248          0
...
TOTAL: 210 runs, 248646000 vectors, 0 mismatches, 0 errors
```

The exit status is 0 only when every run produced vectors and none mismatched. Up to
`--show N` mismatches (default 5) are printed under each row, with operands, the expected
result and flags, and hart's result and flags.

## Reference build

`build.sh` builds `testfloat_gen` against SoftFloat with `SPECIALIZE_TYPE=RISCV`, which gives:

- tininess detected after rounding,
- the RISC-V canonical NaNs (`0x7fc00000`, `0x7ff8000000000000`) for every NaN result,
- RISC-V float-to-integer results for out-of-range values and NaN.

The official SoftFloat 3e release archive has no RISC-V specialization, so the script uses
Berkeley's own repositories (release 3e plus later fixes), pinned to:

| Repository | Commit |
| --- | --- |
| `github.com/ucb-bar/berkeley-softfloat-3` | `32eb3295a3c814c1551f94ebc29eed1dec3cb3b1` |
| `github.com/ucb-bar/berkeley-testfloat-3` | `a9c849f1b0eb0264b626d9686ffae167d996e3be` |

The build goes to `tests/testfloat/build/` (ignored by git), and `build/VERSIONS` records
what was built. Environment overrides: `TESTFLOAT_BUILD_DIR`, `TESTFLOAT_PLATFORM` (a
directory under TestFloat's `build/`, default `Linux-x86_64-GCC`), and `JOBS`.

## What is checked

Vectors come from `testfloat_gen -level N -<rounding> -tininessafter -exact <function>`.
`-exact` makes float-to-integer conversions raise inexact, as RISC-V `fcvt` does.

| Group | Functions |
| --- | --- |
| Arithmetic | `f32_add`, `f32_sub`, `f32_mul`, `f32_div`, `f32_sqrt`, and the `f64_` forms |
| Fused multiply-add | `f32_mulAdd`, `f64_mulAdd`, and the derived `fmsub`, `fnmsub`, `fnmadd` checks |
| Comparisons | `f32_eq` (quiet, RISC-V `feq`), `f32_lt` and `f32_le` (signaling, `flt`/`fle`), and the `f64_` forms |
| Float to integer | `f32_to_i32`, `f32_to_ui32`, `f32_to_i64`, `f32_to_ui64`, and the `f64_` forms |
| Integer to float | `i32_to_f32`, `ui32_to_f32`, `i64_to_f32`, `ui64_to_f32`, and the `_f64` forms |
| Between formats | `f32_to_f64`, `f64_to_f32` |

The rounding modes are `rne`, `rtz`, `rdn`, `rup`, and `rmm` (TestFloat's `-rnear_even`,
`-rminMag`, `-rmin`, `-rmax`, and `-rnear_maxMag`). Every function runs in all five,
including functions whose results do not depend on rounding.

TestFloat has only `mulAdd` (`a * b + c`). The `f32_fmsub`, `f32_fnmsub`, and `f32_fnmadd`
checks (and their `f64_` forms) take the `mulAdd` vectors and negate operands so that each
RISC-V form computes exactly `a * b + c`: `fmsub(a, b, -c)`, `fnmsub(-a, b, c)`, and
`fnmadd(-a, b, -c)`. Sign changes never alter a NaN's signaling bit, so results and flags must
match the `mulAdd` vector exactly.

Level 2 defaults to the core operations: add, sub, mul, div, sqrt, and mulAdd, in both formats.
Name functions to check others.

Not covered by TestFloat: `fmin`/`fmax`, `fclass`, sign injection, and the `fmv` moves.
The unit tests in `crates/hart-core/src/softfloat/tests.rs`, riscv-tests rv64uf/rv64ud, and
ACT4 cover those.

## Commands

```text
hart-testfloat run [--level N] [--jobs N] [--rm LIST] [--gen PATH] [--show N] [FUNCTION...]
hart-testfloat check FUNCTION RM [--show N] < vectors
hart-testfloat list
```

- `run` spawns `testfloat_gen` for every selected function and rounding mode (in parallel,
  `--jobs` defaults to the CPU count) and checks its output. `--rm` takes a comma-separated
  list or `all`. `--gen` (or `TESTFLOAT_GEN`) overrides the `testfloat_gen` path.
- `check` reads vectors from stdin, for example:

  ```sh
  build/bin/testfloat_gen -level 2 -rmin -tininessafter -exact f64_div \
      | ../../target/release/hart-testfloat check f64_div rdn
  ```

- `list` prints every function name `run` and `check` accept.

## Checking the checker

Each of these must make the harness fail:

```sh
G=tests/testfloat/build/bin/testfloat_gen; H=target/release/hart-testfloat
# Corrupt one expected flags byte: 1 mismatch.
$G -level 1 -rnear_even -tininessafter -exact f64_add | sed '100s/ 0[0-9A-F]$/ 1F/' | $H check f64_add rne
# Feed vectors from the wrong rounding mode: thousands of mismatches.
$G -level 1 -rminMag -tininessafter -exact f64_add | $H check f64_add rne
```

Switching the library to tininess-before-rounding (`be < 0` to `be <= 0` in
`softfloat/pack.rs`) produced 24 mismatches in `f64_mul`, 1161 in `f32_mulAdd`, and 7 in
`f64_to_f32` at level 1 in RNE.
