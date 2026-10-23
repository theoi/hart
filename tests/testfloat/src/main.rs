//! Checks hart's soft-float library against Berkeley TestFloat vectors.
//!
//! `testfloat_gen` (built against SoftFloat with the RISC-V specialization)
//! prints one test case per line: the operands, the expected result, and the
//! expected exception flags, all in hexadecimal. This tool feeds the operands
//! through `hart_core::softfloat` and compares the result and the flags bit
//! for bit.
//!
//! ```text
//! hart-testfloat run [--level N] [--n COUNT] [--seed S] [--jobs N] [--rounding LIST] [--gen PATH] [--show N] [FUNCTION...]
//! hart-testfloat check FUNCTION RM [--level N] [--show N] < vectors
//! hart-testfloat list
//! ```
//!
//! See README.md in this directory for details.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use hart_core::softfloat::{self as sf, Flags, RoundingMode};

type Op = fn(&[u64; 3], RoundingMode, &mut Flags) -> u64;

struct Func {
    /// Name accepted on the command line.
    name: &'static str,
    /// The `testfloat_gen` function whose vectors drive this check.
    source: &'static str,
    /// Number of operands.
    arity: usize,
    /// Result width in bits (1 for booleans).
    result_bits: u32,
    op: Op,
}

const F32_SIGN: u64 = 0x8000_0000;
const F64_SIGN: u64 = 0x8000_0000_0000_0000;

macro_rules! func {
    ($name:literal, $source:literal, $arity:literal, $bits:literal, |$x:ident, $rm:ident, $fl:ident| $body:expr) => {
        Func {
            name: $name,
            source: $source,
            arity: $arity,
            result_bits: $bits,
            #[allow(unused_variables)]
            op: |$x: &[u64; 3], $rm: RoundingMode, $fl: &mut Flags| -> u64 { $body },
        }
    };
}

#[rustfmt::skip]
static FUNCS: &[Func] = &[
    func!("ui32_to_f32", "ui32_to_f32", 1, 32, |x, rm, fl| sf::u32_to_f32(x[0] as u32, rm, fl) as u64),
    func!("ui32_to_f64", "ui32_to_f64", 1, 64, |x, rm, fl| sf::u32_to_f64(x[0] as u32, rm, fl)),
    func!("ui64_to_f32", "ui64_to_f32", 1, 32, |x, rm, fl| sf::u64_to_f32(x[0], rm, fl) as u64),
    func!("ui64_to_f64", "ui64_to_f64", 1, 64, |x, rm, fl| sf::u64_to_f64(x[0], rm, fl)),
    func!("i32_to_f32", "i32_to_f32", 1, 32, |x, rm, fl| sf::i32_to_f32(x[0] as i32, rm, fl) as u64),
    func!("i32_to_f64", "i32_to_f64", 1, 64, |x, rm, fl| sf::i32_to_f64(x[0] as i32, rm, fl)),
    func!("i64_to_f32", "i64_to_f32", 1, 32, |x, rm, fl| sf::i64_to_f32(x[0] as i64, rm, fl) as u64),
    func!("i64_to_f64", "i64_to_f64", 1, 64, |x, rm, fl| sf::i64_to_f64(x[0] as i64, rm, fl)),
    func!("f32_to_ui32", "f32_to_ui32", 1, 32, |x, rm, fl| sf::f32_to_u32(x[0] as u32, rm, fl) as u64),
    func!("f32_to_ui64", "f32_to_ui64", 1, 64, |x, rm, fl| sf::f32_to_u64(x[0] as u32, rm, fl)),
    func!("f32_to_i32", "f32_to_i32", 1, 32, |x, rm, fl| sf::f32_to_i32(x[0] as u32, rm, fl) as u32 as u64),
    func!("f32_to_i64", "f32_to_i64", 1, 64, |x, rm, fl| sf::f32_to_i64(x[0] as u32, rm, fl) as u64),
    func!("f32_to_f64", "f32_to_f64", 1, 64, |x, rm, fl| sf::f32_to_f64(x[0] as u32, fl)),
    func!("f32_add", "f32_add", 2, 32, |x, rm, fl| sf::f32_add(x[0] as u32, x[1] as u32, rm, fl) as u64),
    func!("f32_sub", "f32_sub", 2, 32, |x, rm, fl| sf::f32_sub(x[0] as u32, x[1] as u32, rm, fl) as u64),
    func!("f32_mul", "f32_mul", 2, 32, |x, rm, fl| sf::f32_mul(x[0] as u32, x[1] as u32, rm, fl) as u64),
    func!("f32_mulAdd", "f32_mulAdd", 3, 32, |x, rm, fl| sf::f32_fmadd(x[0] as u32, x[1] as u32, x[2] as u32, rm, fl) as u64),
    // The other RISC-V FMA forms, driven by mulAdd vectors with negated operands
    // so that each computes exactly a * b + c.
    func!("f32_fmsub", "f32_mulAdd", 3, 32, |x, rm, fl| sf::f32_fmsub(x[0] as u32, x[1] as u32, (x[2] ^ F32_SIGN) as u32, rm, fl) as u64),
    func!("f32_fnmsub", "f32_mulAdd", 3, 32, |x, rm, fl| sf::f32_fnmsub((x[0] ^ F32_SIGN) as u32, x[1] as u32, x[2] as u32, rm, fl) as u64),
    func!("f32_fnmadd", "f32_mulAdd", 3, 32, |x, rm, fl| sf::f32_fnmadd((x[0] ^ F32_SIGN) as u32, x[1] as u32, (x[2] ^ F32_SIGN) as u32, rm, fl) as u64),
    func!("f32_div", "f32_div", 2, 32, |x, rm, fl| sf::f32_div(x[0] as u32, x[1] as u32, rm, fl) as u64),
    func!("f32_sqrt", "f32_sqrt", 1, 32, |x, rm, fl| sf::f32_sqrt(x[0] as u32, rm, fl) as u64),
    func!("f32_eq", "f32_eq", 2, 1, |x, rm, fl| sf::f32_eq(x[0] as u32, x[1] as u32, fl) as u64),
    func!("f32_le", "f32_le", 2, 1, |x, rm, fl| sf::f32_le(x[0] as u32, x[1] as u32, fl) as u64),
    func!("f32_lt", "f32_lt", 2, 1, |x, rm, fl| sf::f32_lt(x[0] as u32, x[1] as u32, fl) as u64),
    func!("f64_to_ui32", "f64_to_ui32", 1, 32, |x, rm, fl| sf::f64_to_u32(x[0], rm, fl) as u64),
    func!("f64_to_ui64", "f64_to_ui64", 1, 64, |x, rm, fl| sf::f64_to_u64(x[0], rm, fl)),
    func!("f64_to_i32", "f64_to_i32", 1, 32, |x, rm, fl| sf::f64_to_i32(x[0], rm, fl) as u32 as u64),
    func!("f64_to_i64", "f64_to_i64", 1, 64, |x, rm, fl| sf::f64_to_i64(x[0], rm, fl) as u64),
    func!("f64_to_f32", "f64_to_f32", 1, 32, |x, rm, fl| sf::f64_to_f32(x[0], rm, fl) as u64),
    func!("f64_add", "f64_add", 2, 64, |x, rm, fl| sf::f64_add(x[0], x[1], rm, fl)),
    func!("f64_sub", "f64_sub", 2, 64, |x, rm, fl| sf::f64_sub(x[0], x[1], rm, fl)),
    func!("f64_mul", "f64_mul", 2, 64, |x, rm, fl| sf::f64_mul(x[0], x[1], rm, fl)),
    func!("f64_mulAdd", "f64_mulAdd", 3, 64, |x, rm, fl| sf::f64_fmadd(x[0], x[1], x[2], rm, fl)),
    func!("f64_fmsub", "f64_mulAdd", 3, 64, |x, rm, fl| sf::f64_fmsub(x[0], x[1], x[2] ^ F64_SIGN, rm, fl)),
    func!("f64_fnmsub", "f64_mulAdd", 3, 64, |x, rm, fl| sf::f64_fnmsub(x[0] ^ F64_SIGN, x[1], x[2], rm, fl)),
    func!("f64_fnmadd", "f64_mulAdd", 3, 64, |x, rm, fl| sf::f64_fnmadd(x[0] ^ F64_SIGN, x[1], x[2] ^ F64_SIGN, rm, fl)),
    func!("f64_div", "f64_div", 2, 64, |x, rm, fl| sf::f64_div(x[0], x[1], rm, fl)),
    func!("f64_sqrt", "f64_sqrt", 1, 64, |x, rm, fl| sf::f64_sqrt(x[0], rm, fl)),
    func!("f64_eq", "f64_eq", 2, 1, |x, rm, fl| sf::f64_eq(x[0], x[1], fl) as u64),
    func!("f64_le", "f64_le", 2, 1, |x, rm, fl| sf::f64_le(x[0], x[1], fl) as u64),
    func!("f64_lt", "f64_lt", 2, 1, |x, rm, fl| sf::f64_lt(x[0], x[1], fl) as u64),
];

/// The functions checked at level 2 when none are named.
const LEVEL2_DEFAULT: &[&str] = &[
    "f32_add",
    "f32_sub",
    "f32_mul",
    "f32_mulAdd",
    "f32_div",
    "f32_sqrt",
    "f64_add",
    "f64_sub",
    "f64_mul",
    "f64_mulAdd",
    "f64_div",
    "f64_sqrt",
];

/// Rounding modes: (short name, testfloat_gen option).
const ROUNDINGS: &[(&str, &str, RoundingMode)] = &[
    ("rne", "-rnear_even", RoundingMode::NearestEven),
    ("rtz", "-rminMag", RoundingMode::TowardZero),
    ("rdn", "-rmin", RoundingMode::Down),
    ("rup", "-rmax", RoundingMode::Up),
    ("rmm", "-rnear_maxMag", RoundingMode::NearestMaxMag),
];

fn find_func(name: &str) -> Option<&'static Func> {
    FUNCS.iter().find(|f| f.name == name)
}

fn find_rm(name: &str) -> Option<&'static (&'static str, &'static str, RoundingMode)> {
    ROUNDINGS.iter().find(|r| r.0 == name)
}

#[derive(Default)]
struct Outcome {
    cases: u64,
    mismatches: u64,
    /// The first few mismatches, already formatted.
    samples: Vec<String>,
}

fn parse_hex(tok: &[u8]) -> Option<u64> {
    if tok.is_empty() || tok.len() > 16 {
        return None;
    }
    let mut v: u64 = 0;
    for &ch in tok {
        let digit = match ch {
            b'0'..=b'9' => ch - b'0',
            b'a'..=b'f' => ch - b'a' + 10,
            b'A'..=b'F' => ch - b'A' + 10,
            _ => return None,
        };
        v = (v << 4) | digit as u64;
    }
    Some(v)
}

fn hex(v: u64, bits: u32) -> String {
    match bits {
        1 => format!("{v}"),
        32 => format!("{v:08X}"),
        _ => format!("{v:016X}"),
    }
}

/// Checks every vector read from `input` against `func` in rounding mode `rm`.
/// Stops after `limit` vectors when it is set.
fn check_stream(
    func: &Func,
    rm: RoundingMode,
    input: impl Read,
    show: usize,
    limit: Option<u64>,
) -> Result<Outcome, String> {
    let mut reader = BufReader::with_capacity(1 << 20, input);
    let mut line = Vec::with_capacity(128);
    let mut out = Outcome::default();
    let mask = if func.result_bits == 64 { u64::MAX } else { (1u64 << func.result_bits) - 1 };
    while limit.is_none_or(|l| out.cases < l) {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).map_err(|e| format!("read error: {e}"))?;
        if n == 0 {
            break;
        }
        let mut fields = [0u64; 5];
        let mut count = 0;
        for tok in line.split(|c| c.is_ascii_whitespace()).filter(|t| !t.is_empty()) {
            if count == fields.len() {
                count += 1;
                break;
            }
            fields[count] =
                parse_hex(tok).ok_or_else(|| format!("malformed vector line: {:?}", String::from_utf8_lossy(&line)))?;
            count += 1;
        }
        if count == 0 {
            continue;
        }
        if count != func.arity + 2 {
            return Err(format!(
                "expected {} fields, got {count}: {:?}",
                func.arity + 2,
                String::from_utf8_lossy(&line)
            ));
        }
        let mut operands = [0u64; 3];
        operands[..func.arity].copy_from_slice(&fields[..func.arity]);
        let want = fields[func.arity];
        let want_flags = fields[func.arity + 1] as u8;
        let mut fl = Flags::empty();
        let got = (func.op)(&operands, rm, &mut fl) & mask;
        out.cases += 1;
        if got != want || fl.bits() != want_flags {
            out.mismatches += 1;
            if out.samples.len() < show {
                let ops: Vec<String> = operands[..func.arity].iter().map(|&v| format!("{v:X}")).collect();
                out.samples.push(format!(
                    "  {} -> expected {} flags {:02X}, got {} flags {:02X}",
                    ops.join(" "),
                    hex(want, func.result_bits),
                    want_flags,
                    hex(got, func.result_bits),
                    fl.bits()
                ));
            }
        }
    }
    Ok(out)
}

/// Vectors a complete `testfloat_gen -level 1` or `-level 2` run emits for `source`
/// (TestFloat's `genCases_total`). The count depends only on the operand types, not on
/// the rounding mode, `-exact`, or the seed. The values were measured from testfloat_gen's
/// output; the level-2 mulAdd totals are the minimums testfloat_gen reports for `-n`.
fn full_level_count(source: &str, level: u32) -> Option<u64> {
    let (level1, level2): (u64, u64) = if source.starts_with("ui32_") || source.starts_with("i32_") {
        (372, 15_500)
    } else if source.starts_with("ui64_") || source.starts_with("i64_") {
        (756, 63_756)
    } else {
        let (fmt, op) = source.split_once('_')?;
        let single = match fmt {
            "f32" => true,
            "f64" => false,
            _ => return None,
        };
        match op {
            "mulAdd" => (6_133_248, if single { 14_512_627_712 } else { 180_795_884_544 }),
            "add" | "sub" | "mul" | "div" | "eq" | "le" | "lt" => (46_464, if single { 7_496_192 } else { 40_284_288 }),
            "sqrt" | "to_ui32" | "to_ui64" | "to_i32" | "to_i64" | "to_f32" | "to_f64" => {
                if single {
                    (600, 8_800)
                } else {
                    (768, 26_112)
                }
            }
            _ => return None,
        }
    };
    match level {
        1 => Some(level1),
        2 => Some(level2),
        _ => None,
    }
}

/// Options passed through to `testfloat_gen`.
struct GenOptions {
    path: String,
    level: u32,
    /// Stop after this many vectors of `-forever` (level 2) generation.
    count: Option<u64>,
    /// `-seed`: seed for the random half of the cases.
    seed: Option<u64>,
}

/// Runs `testfloat_gen` for one function and rounding mode and checks its output.
fn run_one(gen: &GenOptions, func: &Func, rm: &(&str, &str, RoundingMode), show: usize) -> Result<Outcome, String> {
    // testfloat_gen's own -n can only raise the count above a level's minimum, so a
    // capped run uses -forever (level 2 generation) and stops reading after `count`.
    let mut args = match gen.count {
        Some(_) => vec!["-forever".to_string()],
        None => vec!["-level".to_string(), gen.level.to_string()],
    };
    if let Some(s) = gen.seed {
        args.extend(["-seed".to_string(), s.to_string()]);
    }
    args.extend([rm.1, "-tininessafter", "-exact", func.source].map(String::from));
    let path = gen.path.as_str();
    let mut child = Command::new(path)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {path}: {e}"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let result = check_stream(func, rm.2, stdout, show, gen.count);
    if gen.count.is_some() {
        let _ = child.kill();
    }
    let output = child.wait_with_output().map_err(|e| format!("{path}: {e}"))?;
    if gen.count.is_none() && !output.status.success() {
        return Err(format!("{path} failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).trim()));
    }
    let outcome = result?;
    // A generator that stops early but exits successfully must not pass.
    let expected = match gen.count {
        Some(n) => n,
        None => full_level_count(func.source, gen.level)
            .ok_or_else(|| format!("no expected vector count for {} at level {}", func.source, gen.level))?,
    };
    if outcome.cases != expected {
        return Err(format!("{path} produced {} vectors for {}, expected {expected}", outcome.cases, func.source));
    }
    Ok(outcome)
}

fn default_gen() -> String {
    std::env::var("TESTFLOAT_GEN")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/build/bin/testfloat_gen").to_string())
}

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  hart-testfloat run [--level N] [--n COUNT] [--seed S] [--jobs N] [--rounding LIST] [--gen PATH] [--show N] [FUNCTION...]\n  \
         hart-testfloat check FUNCTION RM [--level N] [--show N] < vectors\n  hart-testfloat list\n\n\
         RM is one of rne, rtz, rdn, rup, rmm; LIST is a comma-separated list of them or `all`."
    );
    ExitCode::from(2)
}

fn cmd_run(args: &[String]) -> ExitCode {
    let mut level = 1u32;
    let mut jobs = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut rms: Vec<&'static (&str, &str, RoundingMode)> = ROUNDINGS.iter().collect();
    let mut gen = default_gen();
    let mut show = 5usize;
    let mut count: Option<u64> = None;
    let mut seed: Option<u64> = None;
    let mut names: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |what: &str| -> Option<String> {
            let v = it.next().cloned();
            if v.is_none() {
                eprintln!("{what} needs a value");
            }
            v
        };
        match arg.as_str() {
            "--level" => match value("--level").and_then(|v| v.parse().ok()) {
                Some(v @ 1..=2) => level = v,
                _ => return usage(),
            },
            "--jobs" => match value("--jobs").and_then(|v| v.parse().ok()) {
                Some(v) if v > 0 => jobs = v,
                _ => return usage(),
            },
            "--n" => match value("--n").and_then(|v| v.parse().ok()) {
                Some(v) if v > 0 => count = Some(v),
                _ => return usage(),
            },
            "--seed" => match value("--seed").and_then(|v| v.parse().ok()) {
                Some(v) => seed = Some(v),
                None => return usage(),
            },
            "--show" => match value("--show").and_then(|v| v.parse().ok()) {
                Some(v) => show = v,
                None => return usage(),
            },
            "--gen" => match value("--gen") {
                Some(v) => gen = v,
                None => return usage(),
            },
            "--rounding" | "--rm" => {
                let Some(list) = value("--rounding") else { return usage() };
                if list == "all" {
                    rms = ROUNDINGS.iter().collect();
                } else {
                    rms.clear();
                    for name in list.split(',') {
                        match find_rm(name) {
                            Some(r) => rms.push(r),
                            None => {
                                eprintln!("unknown rounding mode {name}");
                                return usage();
                            }
                        }
                    }
                }
            }
            s if s.starts_with('-') => {
                eprintln!("unknown option {s}");
                return usage();
            }
            s => names.push(s.to_string()),
        }
    }
    let funcs: Vec<&'static Func> = if names.is_empty() {
        if level == 1 {
            FUNCS.iter().collect()
        } else {
            LEVEL2_DEFAULT.iter().map(|n| find_func(n).expect("known function")).collect()
        }
    } else {
        let mut v = Vec::new();
        for n in &names {
            match find_func(n) {
                Some(f) => v.push(f),
                None => {
                    eprintln!("unknown function {n} (see `hart-testfloat list`)");
                    return ExitCode::from(2);
                }
            }
        }
        v
    };

    let gen = GenOptions { path: gen, level, count, seed };
    let tasks: Vec<(&Func, &(&str, &str, RoundingMode))> =
        funcs.iter().flat_map(|f| rms.iter().map(move |r| (*f, *r))).collect();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result<Outcome, String>>>> = Mutex::new((0..tasks.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(tasks.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(func, rm)) = tasks.get(i) else { break };
                let r = run_one(&gen, func, rm, show);
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    let results = results.into_inner().unwrap();

    let mut stdout = std::io::stdout().lock();
    let mut total_cases = 0u64;
    let mut total_mismatches = 0u64;
    let mut errors = 0usize;
    let mut mode = match count {
        Some(n) => format!("level 2 (-forever, first {n} vectors per run)"),
        None => format!("level {level}"),
    };
    if let Some(s) = seed {
        mode.push_str(&format!(", -seed {s}"));
    }
    let _ = writeln!(stdout, "TestFloat {mode}, testfloat_gen: {}", gen.path);
    let _ = writeln!(stdout, "{:<12} {:<4} {:>12} {:>10}", "function", "rm", "vectors", "mismatches");
    for ((func, rm), r) in tasks.iter().zip(results) {
        match r.expect("every task ran") {
            Ok(o) => {
                total_cases += o.cases;
                total_mismatches += o.mismatches;
                let _ = writeln!(stdout, "{:<12} {:<4} {:>12} {:>10}", func.name, rm.0, o.cases, o.mismatches);
                for s in &o.samples {
                    let _ = writeln!(stdout, "{s}");
                }
            }
            Err(e) => {
                errors += 1;
                let _ = writeln!(stdout, "{:<12} {:<4} ERROR: {e}", func.name, rm.0);
            }
        }
    }
    let _ = writeln!(
        stdout,
        "TOTAL: {} runs, {total_cases} vectors, {total_mismatches} mismatches, {errors} errors",
        tasks.len()
    );
    if total_mismatches == 0 && errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn cmd_check(args: &[String]) -> ExitCode {
    let mut show = 20usize;
    let mut level: Option<u32> = None;
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--show" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => show = v,
                None => return usage(),
            },
            "--level" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v @ 1..=2) => level = Some(v),
                _ => return usage(),
            },
            _ => pos.push(a.as_str()),
        }
    }
    let [name, rm] = pos[..] else { return usage() };
    let Some(func) = find_func(name) else {
        eprintln!("unknown function {name}");
        return ExitCode::from(2);
    };
    let Some(rm) = find_rm(rm) else {
        eprintln!("unknown rounding mode {rm}");
        return usage();
    };
    match check_stream(func, rm.2, std::io::stdin().lock(), show, None) {
        Ok(o) => {
            println!("{} {}: {} vectors, {} mismatches", func.name, rm.0, o.cases, o.mismatches);
            for s in &o.samples {
                println!("{s}");
            }
            let expected = level.and_then(|l| full_level_count(func.source, l));
            if let Some(n) = expected.filter(|&n| n != o.cases) {
                eprintln!("expected {n} vectors for a full level {} run", level.unwrap_or(0));
                return ExitCode::FAILURE;
            }
            if o.mismatches == 0 && o.cases > 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => cmd_run(&args[1..]),
        Some("check") => cmd_check(&args[1..]),
        Some("list") => {
            for f in FUNCS {
                if f.name == f.source {
                    println!("{}", f.name);
                } else {
                    println!("{} (driven by {} vectors)", f.name, f.source);
                }
            }
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_has_full_level_counts() {
        for f in FUNCS {
            for level in [1, 2] {
                assert!(full_level_count(f.source, level).is_some(), "{} level {level}", f.source);
            }
        }
    }

    /// A generator that stops early and exits 0 must fail the run.
    #[test]
    fn truncated_generator_output_fails() {
        let dir = std::env::temp_dir().join(format!("hart-testfloat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("fake_gen.sh");
        std::fs::write(&fake, "#!/bin/sh\nfor i in 1 2 3; do echo 3F800000 3F800000 40000000 00; done\n").unwrap();
        let status = Command::new("chmod").arg("+x").arg(&fake).status().unwrap();
        assert!(status.success());
        let gen = GenOptions { path: fake.to_string_lossy().into_owned(), level: 1, count: None, seed: None };
        let func = find_func("f32_add").unwrap();
        let err = match run_one(&gen, func, find_rm("rne").unwrap(), 0) {
            Ok(o) => panic!("truncated stream passed with {} vectors", o.cases),
            Err(e) => e,
        };
        assert!(err.contains("produced 3 vectors for f32_add, expected 46464"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
