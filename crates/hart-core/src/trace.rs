//! Commit-log tracing in Spike's `--log-commits` format.
//!
//! One line per retired instruction:
//!
//! ```text
//! core   0: 3 0x0000000080000004 (0x02028593) x11 0x0000000080000020
//! core   0: 3 0x0000000080000010 (0x0182b283) x5  0x0000000080001000 mem 0x0000000080000018
//! core   0: 1 0x0000000080002000 (0xc622) mem 0x0000000080003fc8 0x0000000000000001
//! ```
//!
//! Fields: privilege mode before the instruction, pc, instruction bits (4 hex digits for
//! compressed), then register writes (`xN`, `fN`, `cADDR_name`), load addresses
//! (`mem ADDR`), and store addresses with data (`mem ADDR DATA`, data width = store size).
//! Memory addresses are virtual, as in Spike. Traps produce no commit line; interrupts and exceptions
//! are reported separately through [`TraceSink::trap`].

use crate::cpu::{csr, Priv, Trap};
use std::fmt::Write as _;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegWrite {
    X(u8, u64),
    F(u8, u64),
    Csr(u16, u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemWrite {
    pub addr: u64,
    pub value: u64,
    pub size: u8,
}

/// Everything one retired instruction did, in execution order.
#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub mode: u8,
    pub pc: u64,
    pub insn: u32,
    pub len: u8,
    pub regs: Vec<RegWrite>,
    pub loads: Vec<u64>,
    pub stores: Vec<MemWrite>,
}

impl Commit {
    pub fn clear(&mut self) {
        self.regs.clear();
        self.loads.clear();
        self.stores.clear();
    }
}

pub trait TraceSink {
    /// Called once per retired instruction.
    fn commit(&mut self, commit: &Commit);
    /// Called when a trap is taken. `epc` is the trapping pc, `from` the mode before.
    fn trap(&mut self, _trap: &Trap, _epc: u64, _from: Priv) {}
}

/// Append the Spike commit-log line for `c` (without the trailing newline).
pub fn format_commit(c: &Commit, out: &mut String) {
    let _ = write!(out, "core   0: {} 0x{:016x} (", c.mode, c.pc);
    if c.len == 2 {
        let _ = write!(out, "0x{:04x})", c.insn & 0xffff);
    } else {
        let _ = write!(out, "0x{:08x})", c.insn);
    }
    for r in &c.regs {
        match *r {
            RegWrite::X(0, _) => {}
            RegWrite::X(n, v) => {
                let _ = write!(out, " x{n:<2} 0x{v:016x}");
            }
            RegWrite::F(n, v) => {
                let _ = write!(out, " f{n:<2} 0x{v:016x}");
            }
            RegWrite::Csr(a, v) => {
                let _ = write!(out, " c{}_{} 0x{v:016x}", a, csr::name(a));
            }
        }
    }
    for a in &c.loads {
        let _ = write!(out, " mem 0x{a:016x}");
    }
    for s in &c.stores {
        let _ = write!(out, " mem 0x{:016x} ", s.addr);
        let _ = match s.size {
            1 => write!(out, "0x{:02x}", s.value as u8),
            2 => write!(out, "0x{:04x}", s.value as u16),
            4 => write!(out, "0x{:08x}", s.value as u32),
            _ => write!(out, "0x{:016x}", s.value),
        };
    }
}

/// Writes Spike-format commit lines to any `io::Write`.
pub struct SpikeCommitLog<W: Write> {
    out: W,
    line: String,
}

impl<W: Write> SpikeCommitLog<W> {
    pub fn new(out: W) -> Self {
        SpikeCommitLog { out, line: String::with_capacity(128) }
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

impl<W: Write> TraceSink for SpikeCommitLog<W> {
    fn commit(&mut self, c: &Commit) {
        self.line.clear();
        format_commit(c, &mut self.line);
        self.line.push('\n');
        let _ = self.out.write_all(self.line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spike_format() {
        let mut s = String::new();
        let c = Commit {
            mode: 3,
            pc: 0x8000_0010,
            insn: 0x0182_b283,
            len: 4,
            regs: vec![RegWrite::X(5, 0x8000_1000)],
            loads: vec![0x8000_0018],
            stores: vec![],
        };
        format_commit(&c, &mut s);
        assert_eq!(s, "core   0: 3 0x0000000080000010 (0x0182b283) x5  0x0000000080001000 mem 0x0000000080000018");
        s.clear();
        let c = Commit {
            mode: 1,
            pc: 0x8000_2000,
            insn: 0xc622,
            len: 2,
            regs: vec![RegWrite::Csr(0x300, 0xa_0000_0000)],
            loads: vec![],
            stores: vec![MemWrite { addr: 0x8000_3fc8, value: 1, size: 4 }],
        };
        format_commit(&c, &mut s);
        assert_eq!(
            s,
            "core   0: 1 0x0000000080002000 (0xc622) c768_mstatus 0x0000000a00000000 mem 0x0000000080003fc8 0x00000001"
        );
    }
}
