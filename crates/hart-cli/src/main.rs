//! The native `hart` command: boots the virt board with the console on the
//! terminal, or runs a bare-metal test ELF.

mod script;
mod term;

use clap::Parser;
use hart_core::board::{Board, BoardConfig, PowerState, TimeSource, DEFAULT_BOOTARGS};
use hart_core::trace::SpikeCommitLog;
use script::{Script, ScriptEvent, Watch};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};
use term::{EscapeParser, Key, RawMode};

/// Steps per run slice; console output is flushed between slices.
const SLICE: u64 = 1_000_000;
/// Exit status for an instruction limit or timeout, as timeout(1) uses.
const EXIT_LIMIT: u8 = 124;

#[derive(Parser, Debug)]
#[command(name = "hart", version, about = "A from-scratch RV64GC RISC-V computer that boots Linux")]
#[command(after_help = "Console: C-a x exits, C-a h lists the other escapes.")]
struct Args {
    /// Directory with fw_jump.bin and Image, used when --bios or --kernel is not given
    #[arg(long, value_name = "DIR")]
    images: Option<PathBuf>,

    /// Firmware loaded at 0x80000000: OpenSBI fw_jump.bin or fw_dynamic.bin, or an ELF
    #[arg(long, value_name = "FILE")]
    bios: Option<PathBuf>,

    /// Linux kernel Image, loaded at 0x80200000
    #[arg(long, value_name = "FILE")]
    kernel: Option<PathBuf>,

    /// Device tree blob to use instead of the generated one
    #[arg(long, value_name = "FILE")]
    dtb: Option<PathBuf>,

    /// Kernel command line in the generated device tree
    #[arg(long, value_name = "ARGS", default_value = DEFAULT_BOOTARGS)]
    append: String,

    /// Write the generated device tree blob to FILE and exit
    #[arg(long, value_name = "FILE")]
    dump_dtb: Option<PathBuf>,

    /// RAM size in MiB [default: 256, or 2048 with --elf]
    #[arg(short = 'm', long, value_name = "MIB")]
    memory: Option<u64>,

    /// Run a bare-metal ELF on the test profile (HTIF tohost exit; status 0 = pass)
    #[arg(long, value_name = "FILE", conflicts_with_all = ["bios", "kernel", "dtb", "images"])]
    elf: Option<PathBuf>,

    /// Timer source: "real" (host clock) or "insns:N" (one 10 MHz tick per N instructions)
    /// [default: real, or insns:100 with --elf]
    #[arg(long, value_name = "SOURCE")]
    timebase: Option<String>,

    /// Stop after N instructions (exit status 124)
    #[arg(long, value_name = "N")]
    max_insns: Option<u64>,

    /// Stop after SECS seconds of wall-clock time (exit status 124)
    #[arg(long, value_name = "SECS")]
    timeout: Option<f64>,

    /// Write a Spike --log-commits style trace to FILE ("-" for stderr)
    #[arg(long, value_name = "FILE")]
    trace: Option<PathBuf>,

    /// Print the instruction count, run time, and speed on exit
    #[arg(long)]
    stats: bool,

    /// Type each line of FILE at the prompt; exit when the prompt returns after the last line
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,

    /// The shell prompt --script waits for
    #[arg(long, value_name = "TEXT", default_value = "~ # ")]
    prompt: String,

    /// Exit with status 0 once the console prints TEXT
    #[arg(long, value_name = "TEXT")]
    exit_on: Option<String>,
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_timebase(spec: Option<&str>, default: &str) -> Result<TimeSource, String> {
    let spec = spec.unwrap_or(default);
    if spec == "real" {
        let start = Instant::now();
        return Ok(TimeSource::Host(Box::new(move || start.elapsed().as_nanos() as u64)));
    }
    let n = match spec.strip_prefix("insns") {
        Some("") => 100,
        Some(rest) => rest
            .strip_prefix(':')
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("bad --timebase {spec:?}: expected real or insns:N"))?,
        None => return Err(format!("bad --timebase {spec:?}: expected real or insns:N")),
    };
    Ok(TimeSource::Instructions { insns_per_tick: n })
}

/// Guest images, kept so a guest reboot can rebuild the board.
struct Images {
    elf: Option<Vec<u8>>,
    bios: Vec<u8>,
    kernel: Option<Vec<u8>>,
    dtb: Option<Vec<u8>>,
}

impl Images {
    fn load(args: &Args) -> Result<Self, String> {
        if let Some(elf) = &args.elf {
            return Ok(Self { elf: Some(read_file(elf)?), bios: Vec::new(), kernel: None, dtb: None });
        }
        let from_dir = |name: &str| args.images.as_ref().map(|d| d.join(name));
        let bios = args
            .bios
            .clone()
            .or_else(|| from_dir("fw_jump.bin"))
            .ok_or("no firmware: pass --bios FILE, --images DIR, or --elf FILE")?;
        let kernel = args.kernel.clone().or_else(|| from_dir("Image"));
        Ok(Self {
            elf: None,
            bios: read_file(&bios)?,
            kernel: kernel.as_deref().map(read_file).transpose()?,
            dtb: args.dtb.as_deref().map(read_file).transpose()?,
        })
    }

    fn build(&self, args: &Args) -> Result<Board, String> {
        let default_mib = if self.elf.is_some() { 2048 } else { 256 };
        let default_time = if self.elf.is_some() { "insns:100" } else { "real" };
        let config = BoardConfig {
            ram_size: args.memory.unwrap_or(default_mib) << 20,
            bootargs: args.append.clone(),
            time: parse_timebase(args.timebase.as_deref(), default_time)?,
        };
        let mut board = match &self.elf {
            Some(elf) => Board::new_test(config, elf),
            None => Board::new(config, &self.bios, self.kernel.as_deref(), self.dtb.as_deref()),
        }
        .map_err(|e| e.to_string())?;
        if let Some(path) = &args.trace {
            let out: Box<dyn Write> = if path.as_os_str() == "-" {
                Box::new(io::stderr())
            } else {
                Box::new(std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?)
            };
            board.cpu.set_tracer(Some(Box::new(SpikeCommitLog::new(BufWriter::with_capacity(1 << 20, out)))));
        }
        Ok(board)
    }
}

fn spawn_stdin_reader() -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0u8; 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

enum Outcome {
    Exit(u8),
    Message(u8, String),
}

struct Session<'a> {
    args: &'a Args,
    raw: bool,
    escape: EscapeParser,
    script: Option<Script>,
    watch: Option<Watch>,
    started: Instant,
    total_insns: u64,
}

impl Session<'_> {
    fn stats_line(&self, board_insns: u64) -> String {
        let insns = self.total_insns + board_insns;
        let secs = self.started.elapsed().as_secs_f64();
        format!("hart: {insns} instructions in {secs:.2} s ({:.1} MIPS)", insns as f64 / secs.max(1e-9) / 1e6)
    }

    fn print_notice(&self, msg: &str) {
        let mut err = io::stderr();
        let _ = write!(err, "{}{msg}\r\n", if self.raw { "\r\n" } else { "" });
    }

    /// Handles host input. Returns Some on quit.
    fn handle_input(&mut self, board: &mut Board, bytes: &[u8]) -> Option<Outcome> {
        if !self.raw {
            board.push_input(bytes);
            return None;
        }
        let mut guest = Vec::with_capacity(bytes.len());
        for &b in bytes {
            match self.escape.feed(b) {
                Some(Key::Guest(g)) => guest.push(g),
                Some(Key::Quit) => return Some(Outcome::Exit(0)),
                Some(Key::Stats) => self.print_notice(&self.stats_line(board.instret())),
                Some(Key::Help) => self.print_notice(term::ESCAPE_HELP.trim_end()),
                None => {}
            }
        }
        if !guest.is_empty() {
            board.push_input(&guest);
        }
        None
    }

    fn run(&mut self, images: &Images, input: &Receiver<Vec<u8>>) -> Result<Outcome, String> {
        let mut board = images.build(self.args)?;
        let mut stdout = io::stdout().lock();
        let mut out = Vec::new();
        let mut input_open = true;
        let deadline = self.args.timeout.map(|s| self.started + Duration::from_secs_f64(s));
        loop {
            while input_open {
                match input.try_recv() {
                    Ok(bytes) => {
                        if let Some(outcome) = self.handle_input(&mut board, &bytes) {
                            return Ok(outcome);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => input_open = false,
                }
            }

            let mut slice = SLICE;
            if let Some(max) = self.args.max_insns {
                let done = self.total_insns + board.instret();
                if done >= max {
                    return Ok(Outcome::Message(EXIT_LIMIT, format!("instruction limit ({max}) reached")));
                }
                slice = slice.min(max - done);
            }
            board.run(slice);

            out.clear();
            board.drain_output(&mut out);
            if !out.is_empty() {
                let _ = stdout.write_all(&out);
                let _ = stdout.flush();
            }
            if let Some(watch) = &mut self.watch {
                if watch.seen(&out) {
                    self.total_insns += board.instret();
                    return Ok(Outcome::Exit(0));
                }
            }
            if let Some(script) = &mut self.script {
                match script.on_output(&out) {
                    Some(ScriptEvent::Type(line)) => board.push_input(&line),
                    Some(ScriptEvent::Finished) => {
                        self.total_insns += board.instret();
                        return Ok(Outcome::Exit(0));
                    }
                    None => {}
                }
            }

            match board.power_state() {
                PowerState::Running => {}
                PowerState::PowerOff => {
                    self.total_insns += board.instret();
                    return Ok(Outcome::Exit(0));
                }
                PowerState::Exit(0) => {
                    self.total_insns += board.instret();
                    return Ok(Outcome::Exit(0));
                }
                PowerState::Exit(code) => {
                    self.total_insns += board.instret();
                    return Ok(Outcome::Message(1, format!("guest exited with code {code}")));
                }
                PowerState::Reboot => {
                    self.total_insns += board.instret();
                    drop(board);
                    board = images.build(self.args)?;
                    continue;
                }
            }

            if let Some(deadline) = deadline {
                if Instant::now() >= deadline {
                    self.total_insns += board.instret();
                    return Ok(Outcome::Message(EXIT_LIMIT, "timeout reached".to_string()));
                }
            }

            if board.is_waiting() {
                // Sleep until the next timer interrupt or host input.
                let wait = board.ns_until_timer().map_or(Duration::from_millis(50), Duration::from_nanos);
                let wait = wait.min(Duration::from_millis(50));
                if !input_open {
                    std::thread::sleep(wait);
                    continue;
                }
                match input.recv_timeout(wait) {
                    Ok(bytes) => {
                        if let Some(outcome) = self.handle_input(&mut board, &bytes) {
                            return Ok(outcome);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => input_open = false,
                }
            }
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    match real_main(&args) {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("hart: {msg}");
            ExitCode::from(2)
        }
    }
}

fn real_main(args: &Args) -> Result<u8, String> {
    if let Some(path) = &args.dump_dtb {
        let images = Images::load(args).ok();
        let board = match images {
            Some(images) if images.elf.is_none() => images.build(args)?,
            _ => Images { elf: None, bios: Vec::new(), kernel: None, dtb: None }.build(args)?,
        };
        std::fs::write(path, board.dtb()).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(0);
    }
    let images = Images::load(args)?;
    let script = match &args.script {
        Some(path) => Some(Script::new(
            &String::from_utf8(read_file(path)?).map_err(|_| format!("{}: not UTF-8", path.display()))?,
            &args.prompt,
        )),
        None => None,
    };
    let input = spawn_stdin_reader();
    let raw_mode = if args.elf.is_none() { RawMode::enable() } else { None };
    let mut session = Session {
        args,
        raw: raw_mode.is_some(),
        escape: EscapeParser::default(),
        script,
        watch: args.exit_on.as_deref().map(Watch::new),
        started: Instant::now(),
        total_insns: 0,
    };
    let outcome = session.run(&images, &input);
    let raw = session.raw;
    drop(raw_mode);
    if raw {
        eprintln!();
    }
    let outcome = outcome?;
    if args.stats {
        eprintln!("{}", session.stats_line(0));
    }
    Ok(match outcome {
        Outcome::Exit(code) => code,
        Outcome::Message(code, msg) => {
            eprintln!("hart: {msg}");
            code
        }
    })
}
