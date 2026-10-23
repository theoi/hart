//! Guest image loading: raw binaries and ELF64 little-endian RISC-V files
//! (loadable segments, entry point, and symbol lookup for HTIF
//! `tohost`/`fromhost`).

use crate::bus::Bus;
use std::fmt;

const EM_RISCV: u16 = 243;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    NotRiscv(u16),
    UnsupportedType(u16),
    BadSegment(usize),
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "ELF file is truncated"),
            Self::BadMagic => write!(f, "not an ELF file"),
            Self::Not64Bit => write!(f, "not a 64-bit ELF file"),
            Self::NotLittleEndian => write!(f, "not a little-endian ELF file"),
            Self::NotRiscv(m) => write!(f, "ELF machine {m} is not RISC-V"),
            Self::UnsupportedType(t) => write!(f, "unsupported ELF type {t}"),
            Self::BadSegment(i) => write!(f, "program header {i} is out of bounds"),
        }
    }
}

impl std::error::Error for ElfError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfKind {
    Executable,
    /// Position independent (ET_DYN), such as OpenSBI's `fw_jump.elf`.
    PositionIndependent,
}

/// A PT_LOAD segment. Memory beyond `data.len()` up to `mem_size` is zero.
#[derive(Clone, Debug)]
pub struct Segment<'a> {
    pub paddr: u64,
    pub vaddr: u64,
    pub data: &'a [u8],
    pub mem_size: u64,
}

#[derive(Clone, Debug)]
pub struct Elf<'a> {
    bytes: &'a [u8],
    pub kind: ElfKind,
    pub entry: u64,
    pub segments: Vec<Segment<'a>>,
    symtab: Option<(usize, usize, usize, usize)>,
}

fn u16_at(b: &[u8], off: usize) -> Result<u16, ElfError> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or(ElfError::Truncated)
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, ElfError> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or(ElfError::Truncated)
}

fn u64_at(b: &[u8], off: usize) -> Result<u64, ElfError> {
    b.get(off..off + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or(ElfError::Truncated)
}

/// A failure to place an image in guest RAM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Elf(ElfError),
    /// `[addr, addr+len)` is not entirely inside RAM.
    OutsideRam {
        addr: u64,
        len: u64,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Elf(e) => e.fmt(f),
            Self::OutsideRam { addr, len } => {
                write!(f, "image range {addr:#x}..{:#x} is outside RAM", addr.wrapping_add(*len))
            }
        }
    }
}

impl std::error::Error for LoadError {}

impl From<ElfError> for LoadError {
    fn from(e: ElfError) -> Self {
        Self::Elf(e)
    }
}

/// Copies a raw binary into RAM at `addr`.
pub fn load_raw(bus: &mut Bus, addr: u64, bytes: &[u8]) -> Result<(), LoadError> {
    bus.write_bytes(addr, bytes).map_err(|_| LoadError::OutsideRam { addr, len: bytes.len() as u64 })
}

/// Where an ELF image was placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedElf {
    /// Entry point, including `bias`.
    pub entry: u64,
    /// Added to every physical address and symbol in the file.
    pub bias: u64,
    /// Lowest and highest (exclusive) physical addresses written.
    pub start: u64,
    pub end: u64,
    pub tohost: Option<u64>,
    pub fromhost: Option<u64>,
}

/// Loads every PT_LOAD segment at its physical address and zero-fills the
/// rest of each segment. A position-independent image linked at address 0
/// (such as OpenSBI's `fw_jump.elf`) is placed at the start of RAM.
pub fn load_elf(bus: &mut Bus, bytes: &[u8]) -> Result<LoadedElf, LoadError> {
    let elf = Elf::parse(bytes)?;
    let lowest = elf.lowest_paddr().unwrap_or(0);
    let bias = if elf.kind == ElfKind::PositionIndependent && !bus.is_ram(lowest, 1) {
        bus.ram.base().wrapping_sub(lowest)
    } else {
        0
    };
    load_elf_at(bus, &elf, bias)
}

/// Loads `elf` with every physical address shifted by `bias`.
pub fn load_elf_at(bus: &mut Bus, elf: &Elf<'_>, bias: u64) -> Result<LoadedElf, LoadError> {
    let mut start = u64::MAX;
    let mut end = 0;
    for seg in &elf.segments {
        let addr = seg.paddr.wrapping_add(bias);
        if seg.mem_size == 0 {
            continue;
        }
        if !bus.is_ram(addr, seg.mem_size) {
            return Err(LoadError::OutsideRam { addr, len: seg.mem_size });
        }
        load_raw(bus, addr, seg.data)?;
        let zeros = vec![0u8; (seg.mem_size - seg.data.len() as u64) as usize];
        load_raw(bus, addr + seg.data.len() as u64, &zeros)?;
        start = start.min(addr);
        end = end.max(addr + seg.mem_size);
    }
    let sym = |name| elf.symbol(name).map(|v| v.wrapping_add(bias));
    Ok(LoadedElf {
        entry: elf.entry.wrapping_add(bias),
        bias,
        start: if start == u64::MAX { 0 } else { start },
        end,
        tohost: sym("tohost"),
        fromhost: sym("fromhost"),
    })
}

/// Whether `bytes` starts with the ELF magic number.
pub fn is_elf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x7fELF")
}

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ElfError> {
        if bytes.len() < 64 {
            return Err(if is_elf(bytes) { ElfError::Truncated } else { ElfError::BadMagic });
        }
        if !is_elf(bytes) {
            return Err(ElfError::BadMagic);
        }
        if bytes[4] != 2 {
            return Err(ElfError::Not64Bit);
        }
        if bytes[5] != 1 {
            return Err(ElfError::NotLittleEndian);
        }
        let kind = match u16_at(bytes, 16)? {
            ET_EXEC => ElfKind::Executable,
            ET_DYN => ElfKind::PositionIndependent,
            t => return Err(ElfError::UnsupportedType(t)),
        };
        let machine = u16_at(bytes, 18)?;
        if machine != EM_RISCV {
            return Err(ElfError::NotRiscv(machine));
        }
        let entry = u64_at(bytes, 24)?;
        let phoff = u64_at(bytes, 32)? as usize;
        let shoff = u64_at(bytes, 40)? as usize;
        let phentsize = usize::from(u16_at(bytes, 54)?);
        let phnum = usize::from(u16_at(bytes, 56)?);
        let shentsize = usize::from(u16_at(bytes, 58)?);
        let shnum = usize::from(u16_at(bytes, 60)?);

        let mut segments = Vec::new();
        for i in 0..phnum {
            let ph = phoff + i * phentsize;
            if u32_at(bytes, ph)? != PT_LOAD {
                continue;
            }
            let offset = u64_at(bytes, ph + 8)? as usize;
            let vaddr = u64_at(bytes, ph + 16)?;
            let paddr = u64_at(bytes, ph + 24)?;
            let file_size = u64_at(bytes, ph + 32)? as usize;
            let mem_size = u64_at(bytes, ph + 40)?;
            let data =
                offset.checked_add(file_size).and_then(|end| bytes.get(offset..end)).ok_or(ElfError::BadSegment(i))?;
            if (file_size as u64) > mem_size {
                return Err(ElfError::BadSegment(i));
            }
            segments.push(Segment { paddr, vaddr, data, mem_size });
        }

        let mut symtab = None;
        if shoff != 0 && shentsize >= 64 {
            for i in 0..shnum {
                let sh = shoff + i * shentsize;
                if u32_at(bytes, sh + 4)? != SHT_SYMTAB {
                    continue;
                }
                let sym_off = u64_at(bytes, sh + 24)? as usize;
                let sym_size = u64_at(bytes, sh + 32)? as usize;
                let link = u32_at(bytes, sh + 40)? as usize;
                let str_sh = shoff + link * shentsize;
                let str_off = u64_at(bytes, str_sh + 24)? as usize;
                let str_size = u64_at(bytes, str_sh + 32)? as usize;
                if sym_off.saturating_add(sym_size) <= bytes.len() && str_off.saturating_add(str_size) <= bytes.len() {
                    symtab = Some((sym_off, sym_size, str_off, str_size));
                }
                break;
            }
        }

        Ok(Self { bytes, kind, entry, segments, symtab })
    }

    /// The value of the first symbol named `name`.
    pub fn symbol(&self, name: &str) -> Option<u64> {
        let (sym_off, sym_size, str_off, str_size) = self.symtab?;
        let strtab = &self.bytes[str_off..str_off + str_size];
        let symbols = &self.bytes[sym_off..sym_off + sym_size];
        for sym in symbols.chunks_exact(24) {
            let name_off = u32::from_le_bytes(sym[0..4].try_into().unwrap()) as usize;
            let Some(rest) = strtab.get(name_off..) else {
                continue;
            };
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            if &rest[..end] == name.as_bytes() {
                return Some(u64::from_le_bytes(sym[8..16].try_into().unwrap()));
            }
        }
        None
    }

    /// The lowest physical address any segment occupies.
    pub fn lowest_paddr(&self) -> Option<u64> {
        self.segments.iter().map(|s| s.paddr).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a small ELF with one PT_LOAD segment and a symbol table
    /// containing `tohost`.
    fn sample(kind: u16, machine: u16) -> Vec<u8> {
        let code: &[u8] = &[0x13, 0, 0, 0, 0x6f, 0, 0, 0];
        let strtab: &[u8] = b"\0start\0tohost\0";
        let mut symtab = vec![0u8; 24];
        for (name, value) in [(1u32, 0x8000_0000u64), (7, 0x8000_1000)] {
            let mut s = vec![0u8; 24];
            s[0..4].copy_from_slice(&name.to_le_bytes());
            s[8..16].copy_from_slice(&value.to_le_bytes());
            symtab.extend(s);
        }
        let ph_off = 64usize;
        let code_off = ph_off + 56;
        let sym_off = code_off + code.len();
        let str_off = sym_off + symtab.len();
        let sh_off = (str_off + strtab.len() + 7) & !7;

        let mut b = vec![0u8; sh_off + 3 * 64];
        b[0..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[6] = 1;
        b[16..18].copy_from_slice(&kind.to_le_bytes());
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        b[24..32].copy_from_slice(&0x8000_0000u64.to_le_bytes());
        b[32..40].copy_from_slice(&(ph_off as u64).to_le_bytes());
        b[40..48].copy_from_slice(&(sh_off as u64).to_le_bytes());
        b[52..54].copy_from_slice(&64u16.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        b[58..60].copy_from_slice(&64u16.to_le_bytes());
        b[60..62].copy_from_slice(&3u16.to_le_bytes());

        let ph = ph_off;
        b[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        b[ph + 8..ph + 16].copy_from_slice(&(code_off as u64).to_le_bytes());
        b[ph + 16..ph + 24].copy_from_slice(&0xffff_ffff_8000_0000u64.to_le_bytes());
        b[ph + 24..ph + 32].copy_from_slice(&0x8000_0000u64.to_le_bytes());
        b[ph + 32..ph + 40].copy_from_slice(&(code.len() as u64).to_le_bytes());
        b[ph + 40..ph + 48].copy_from_slice(&0x2000u64.to_le_bytes());

        b[code_off..sym_off].copy_from_slice(code);
        b[sym_off..str_off].copy_from_slice(&symtab);
        b[str_off..str_off + strtab.len()].copy_from_slice(strtab);

        let sh1 = sh_off + 64;
        b[sh1 + 4..sh1 + 8].copy_from_slice(&SHT_SYMTAB.to_le_bytes());
        b[sh1 + 24..sh1 + 32].copy_from_slice(&(sym_off as u64).to_le_bytes());
        b[sh1 + 32..sh1 + 40].copy_from_slice(&(symtab.len() as u64).to_le_bytes());
        b[sh1 + 40..sh1 + 44].copy_from_slice(&2u32.to_le_bytes());
        let sh2 = sh_off + 128;
        b[sh2 + 4..sh2 + 8].copy_from_slice(&3u32.to_le_bytes());
        b[sh2 + 24..sh2 + 32].copy_from_slice(&(str_off as u64).to_le_bytes());
        b[sh2 + 32..sh2 + 40].copy_from_slice(&(strtab.len() as u64).to_le_bytes());
        b
    }

    #[test]
    fn parses_segments_entry_and_symbols() {
        let bytes = sample(ET_EXEC, EM_RISCV);
        let elf = Elf::parse(&bytes).unwrap();
        assert_eq!(elf.kind, ElfKind::Executable);
        assert_eq!(elf.entry, 0x8000_0000);
        assert_eq!(elf.segments.len(), 1);
        let s = &elf.segments[0];
        assert_eq!(s.paddr, 0x8000_0000);
        assert_eq!(s.vaddr, 0xffff_ffff_8000_0000);
        assert_eq!(s.data, &[0x13, 0, 0, 0, 0x6f, 0, 0, 0]);
        assert_eq!(s.mem_size, 0x2000);
        assert_eq!(elf.symbol("tohost"), Some(0x8000_1000));
        assert_eq!(elf.symbol("start"), Some(0x8000_0000));
        assert_eq!(elf.symbol("fromhost"), None);
        assert_eq!(elf.symbol("toho"), None);
        assert_eq!(elf.lowest_paddr(), Some(0x8000_0000));
    }

    #[test]
    fn position_independent_kind() {
        let bytes = sample(ET_DYN, EM_RISCV);
        assert_eq!(Elf::parse(&bytes).unwrap().kind, ElfKind::PositionIndependent);
    }

    #[test]
    fn load_elf_places_segments_and_symbols() {
        let bytes = sample(ET_EXEC, EM_RISCV);
        let mut bus = Bus::new(0x8000_0000, 0x4000);
        bus.write_bytes(0x8000_1000, &[0xff; 16]).unwrap();
        let loaded = load_elf(&mut bus, &bytes).unwrap();
        assert_eq!(loaded.entry, 0x8000_0000);
        assert_eq!(loaded.bias, 0);
        assert_eq!((loaded.start, loaded.end), (0x8000_0000, 0x8000_2000));
        assert_eq!(loaded.tohost, Some(0x8000_1000));
        assert_eq!(loaded.fromhost, None);
        assert_eq!(bus.read(0x8000_0004, 4), Ok(0x6f));
        assert_eq!(bus.read(0x8000_1000, 8), Ok(0), "bss is zero-filled");
    }

    #[test]
    fn position_independent_image_at_zero_goes_to_ram_base() {
        let mut bytes = sample(ET_DYN, EM_RISCV);
        // Link the segment and entry at 0, like OpenSBI's fw_jump.elf.
        bytes[24..32].copy_from_slice(&0u64.to_le_bytes());
        bytes[64 + 24..64 + 32].copy_from_slice(&0u64.to_le_bytes());
        let mut bus = Bus::new(0x8000_0000, 0x4000);
        let loaded = load_elf(&mut bus, &bytes).unwrap();
        assert_eq!(loaded.bias, 0x8000_0000);
        assert_eq!(loaded.entry, 0x8000_0000);
        assert_eq!(bus.read(0x8000_0000, 4), Ok(0x13));
    }

    #[test]
    fn load_outside_ram_fails() {
        let bytes = sample(ET_EXEC, EM_RISCV);
        let mut bus = Bus::new(0x8000_0000, 0x1000);
        assert_eq!(load_elf(&mut bus, &bytes).unwrap_err(), LoadError::OutsideRam { addr: 0x8000_0000, len: 0x2000 });
        assert!(load_raw(&mut bus, 0x7fff_ffff, &[1]).is_err());
    }

    #[test]
    fn rejects_bad_inputs() {
        assert_eq!(Elf::parse(b"hello").unwrap_err(), ElfError::BadMagic);
        assert_eq!(Elf::parse(b"\x7fELF").unwrap_err(), ElfError::Truncated);
        let bytes = sample(ET_EXEC, 62);
        assert_eq!(Elf::parse(&bytes).unwrap_err(), ElfError::NotRiscv(62));
        let mut bytes = sample(ET_EXEC, EM_RISCV);
        bytes[4] = 1;
        assert_eq!(Elf::parse(&bytes).unwrap_err(), ElfError::Not64Bit);
        let mut bytes = sample(ET_EXEC, EM_RISCV);
        bytes[64 + 32..64 + 40].copy_from_slice(&0x10_0000u64.to_le_bytes());
        assert_eq!(Elf::parse(&bytes).unwrap_err(), ElfError::BadSegment(0));
        let bytes = sample(1, EM_RISCV);
        assert_eq!(Elf::parse(&bytes).unwrap_err(), ElfError::UnsupportedType(1));
    }
}
