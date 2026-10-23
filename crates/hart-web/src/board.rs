//! The real backend: the hart-core virt board, timed by the host clock.

use std::cell::Cell;
use std::rc::Rc;

use hart_core::board::{Board, BoardConfig, PowerState, TimeSource, DEFAULT_BOOTARGS};

use crate::RunState;

pub struct BoardMachine {
    board: Board,
    /// Host time in nanoseconds, updated by each `run` call and read by the board's clock.
    now_ns: Rc<Cell<u64>>,
}

impl BoardMachine {
    pub const NAME: &'static str = "hart-core";

    pub fn new(
        firmware: &[u8],
        kernel: &[u8],
        dtb: Option<&[u8]>,
        ram_mib: u32,
        bootargs: Option<&str>,
    ) -> Result<Self, String> {
        let now_ns = Rc::new(Cell::new(0));
        let clock = Rc::clone(&now_ns);
        let config = BoardConfig {
            ram_size: u64::from(ram_mib) << 20,
            bootargs: bootargs.unwrap_or(DEFAULT_BOOTARGS).to_string(),
            time: TimeSource::Host(Box::new(move || clock.get())),
        };
        let board = Board::new(config, firmware, Some(kernel), dtb).map_err(|e| e.to_string())?;
        Ok(Self { board, now_ns })
    }

    pub fn run(&mut self, max_insns: u32, now_ms: f64) -> u32 {
        // performance.now() is monotonic, but never let a bad caller move time backwards.
        let ns = (now_ms * 1e6) as u64;
        if ns > self.now_ns.get() {
            self.now_ns.set(ns);
        }
        self.board.run(u64::from(max_insns)) as u32
    }

    pub fn push_input(&mut self, bytes: &[u8]) {
        self.board.push_input(bytes);
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        self.board.take_output()
    }

    pub fn instret(&self) -> u64 {
        self.board.instret()
    }

    pub fn state(&self) -> RunState {
        match self.board.power_state() {
            PowerState::Running if self.board.is_waiting() => RunState::Idle,
            PowerState::Running => RunState::Running,
            PowerState::Reboot => RunState::RebootRequested,
            PowerState::PowerOff | PowerState::Exit(_) => RunState::PoweredOff,
        }
    }

    pub fn idle_ms(&mut self) -> f64 {
        self.board.ns_until_timer().map_or(-1.0, |ns| ns as f64 / 1e6)
    }
}
