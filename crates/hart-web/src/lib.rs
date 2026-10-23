//! WebAssembly bindings for hart.
//!
//! This crate is a thin wrapper and owns no emulation logic. JavaScript drives the machine
//! in time slices from a Web Worker, feeds console input, and drains console output between
//! slices.

use wasm_bindgen::prelude::*;

#[cfg(not(feature = "stub"))]
mod board;
#[cfg(feature = "stub")]
mod stub;

#[cfg(not(feature = "stub"))]
use board::BoardMachine as Backend;
#[cfg(feature = "stub")]
use stub::StubMachine as Backend;

/// Run state reported by [`Machine::state`].
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Executing instructions.
    Running = 0,
    /// Waiting for an interrupt with nothing to do; the caller may sleep.
    Idle = 1,
    /// The guest powered the machine off.
    PoweredOff = 2,
    /// The guest asked for a reboot; the caller should recreate the machine.
    RebootRequested = 3,
}

/// The hart-core version string.
#[wasm_bindgen]
pub fn version() -> String {
    hart_core::VERSION.to_string()
}

/// A hart machine (CPU, RAM, and devices) booted from guest images.
#[wasm_bindgen]
pub struct Machine {
    inner: Backend,
}

#[wasm_bindgen]
impl Machine {
    /// Builds a machine from guest images.
    ///
    /// `firmware` is loaded at the start of RAM and `kernel` at the board's kernel offset.
    /// When `dtb` is `None` the board generates its own device tree; `bootargs` then sets
    /// the kernel command line.
    #[wasm_bindgen(constructor)]
    pub fn new(
        firmware: &[u8],
        kernel: &[u8],
        dtb: Option<Vec<u8>>,
        ram_mib: u32,
        bootargs: Option<String>,
    ) -> Result<Machine, JsError> {
        let inner = Backend::new(firmware, kernel, dtb.as_deref(), ram_mib, bootargs.as_deref())
            .map_err(|e| JsError::new(&e))?;
        Ok(Machine { inner })
    }

    /// Name of the compiled-in backend, for display and diagnostics.
    pub fn backend() -> String {
        Backend::NAME.to_string()
    }

    /// Executes up to `max_insns` instructions and returns how many retired.
    ///
    /// `now_ms` is the host's monotonic clock in milliseconds, from which the machine
    /// derives its timer. Returns early when the guest goes idle or stops.
    pub fn run(&mut self, max_insns: u32, now_ms: f64) -> u32 {
        self.inner.run(max_insns, now_ms)
    }

    /// Queues bytes typed by the user for the console UART.
    pub fn push_input(&mut self, bytes: &[u8]) {
        self.inner.push_input(bytes);
    }

    /// Returns and clears the bytes the guest wrote to the console UART.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.inner.take_output()
    }

    /// Total instructions retired since the machine was created.
    pub fn instret(&self) -> f64 {
        self.inner.instret() as f64
    }

    pub fn state(&self) -> RunState {
        self.inner.state()
    }

    pub fn powered_off(&self) -> bool {
        self.inner.state() == RunState::PoweredOff
    }
}
