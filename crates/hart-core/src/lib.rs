//! hart-core: a from-scratch RV64GC hart, its memory system, and a virt-style platform.
//!
//! This crate has no third-party dependencies. See `docs/architecture.md` for the module
//! map and who owns what.

pub mod board;
pub mod bus;
pub mod cpu;
pub mod devices;
pub mod fdt;
pub mod irq;
pub mod loader;
pub mod softfloat;
pub mod trace;

pub use bus::{Bus, BusFault, Device, DeviceId};
pub use cpu::{Cpu, Exception, ExceptionCause, Interrupt, Priv, Trap};
pub use irq::{Halt, Wires};

/// The crate version, for banners and the web build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
