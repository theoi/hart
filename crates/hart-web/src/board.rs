//! The real backend: the hart-core virt board.
//!
//! hart-core's board API is not implemented yet, so construction fails with a clear
//! message. The web front end uses the `stub` feature until it lands.

use crate::RunState;

pub struct BoardMachine {
    instret: u64,
}

impl BoardMachine {
    pub const NAME: &'static str = "hart-core";

    pub fn new(
        _firmware: &[u8],
        _kernel: &[u8],
        _dtb: Option<&[u8]>,
        _ram_mib: u32,
        _bootargs: Option<&str>,
    ) -> Result<Self, String> {
        Err(format!("hart-core {} has no board yet; build hart-web with the `stub` feature", hart_core::VERSION))
    }

    pub fn run(&mut self, _max_insns: u32, _now_ms: f64) -> u32 {
        0
    }

    pub fn push_input(&mut self, _bytes: &[u8]) {}

    pub fn take_output(&mut self) -> Vec<u8> {
        Vec::new()
    }

    pub fn instret(&self) -> u64 {
        self.instret
    }

    pub fn state(&self) -> RunState {
        RunState::PoweredOff
    }
}
