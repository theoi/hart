//! Platform devices: CLINT, PLIC, NS16550A UART, syscon, and the HTIF test device.
//! Owned by the platform builder. Each implements [`crate::bus::Device`].

pub mod htif;
