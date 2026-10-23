//! The physical memory bus: one RAM region plus memory-mapped devices.

use crate::cpu::decode::Insn;
use crate::irq::Wires;
use core::any::Any;

/// A failed physical access. The CPU turns it into an access-fault exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusFault;

/// A memory-mapped device.
///
/// `offset` is relative to the device's base address. `size` is 1, 2, 4, or 8, and the
/// access is naturally aligned: misaligned accesses to device regions raise access faults
/// before reaching the device. Read values are returned zero-extended in the low `size`
/// bytes; written values carry the data in the low `size` bytes.
pub trait Device: Any {
    fn read(&mut self, offset: u64, size: u8, wires: &mut Wires) -> Result<u64, BusFault>;
    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault>;
    /// Called by the board's run loop between instruction batches, after `wires.mtime`
    /// has been advanced. Devices update interrupt lines here.
    fn tick(&mut self, _wires: &mut Wires) {}
}

/// Handle returned by [`Bus::add_device`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(usize);

/// A decoded instruction cached for one 2-byte-aligned RAM address. `raw == 0` marks
/// an empty slot (the all-zero encoding is always illegal, so it is never cached).
#[derive(Clone, Copy, Debug)]
pub struct CodeSlot {
    pub insn: Insn,
    pub raw: u32,
}

const EMPTY_SLOT: CodeSlot = CodeSlot { insn: Insn::ILLEGAL, raw: 0 };
/// Slots per 4 KiB page.
pub const CODE_SLOTS: usize = 2048;
type CodePage = Box<[CodeSlot; CODE_SLOTS]>;
/// Freed decoded pages kept for reuse.
const SPARE_PAGES: usize = 64;

/// Main memory, plus a cache of decoded instructions for each RAM page that has been
/// executed. Every write into a page drops that page's decoded instructions, so the
/// cache never holds stale code.
pub struct Ram {
    base: u64,
    data: Vec<u8>,
    code: Vec<Option<CodePage>>,
    spare: Vec<CodePage>,
}

impl Ram {
    pub fn new(base: u64, size: usize) -> Self {
        let pages = size.div_ceil(4096);
        Ram { base, data: vec![0; size], code: (0..pages).map(|_| None).collect(), spare: Vec::new() }
    }

    /// The cached decode for the instruction at byte offset `off`, if any.
    #[inline(always)]
    pub(crate) fn cached_code(&self, off: usize) -> Option<CodeSlot> {
        let page = self.code[off >> 12].as_ref()?;
        let slot = page[(off & 0xfff) >> 1];
        (slot.raw != 0).then_some(slot)
    }

    /// Cache the decode of the instruction at byte offset `off`. The instruction must
    /// lie entirely inside one page.
    pub(crate) fn cache_code(&mut self, off: usize, slot: CodeSlot) {
        let p = off >> 12;
        if self.code[p].is_none() {
            let page = match self.spare.pop() {
                Some(mut page) => {
                    page.fill(EMPTY_SLOT);
                    page
                }
                None => Box::new([EMPTY_SLOT; CODE_SLOTS]),
            };
            self.code[p] = Some(page);
        }
        if let Some(page) = self.code[p].as_mut() {
            page[(off & 0xfff) >> 1] = slot;
        }
    }

    /// Drop decoded instructions for the pages touched by a write of `len` bytes at `off`.
    #[inline(always)]
    fn invalidate(&mut self, off: usize, len: usize) {
        let first = off >> 12;
        if self.code[first].is_some() {
            self.drop_code(first);
        }
        let last = (off + len - 1) >> 12;
        if last != first && self.code[last].is_some() {
            self.drop_code(last);
        }
    }

    #[cold]
    fn drop_code(&mut self, page: usize) {
        if let Some(p) = self.code[page].take() {
            if self.spare.len() < SPARE_PAGES {
                self.spare.push(p);
            }
        }
    }

    /// Drop all decoded instructions.
    pub fn flush_code(&mut self) {
        for p in 0..self.code.len() {
            if self.code[p].is_some() {
                self.drop_code(p);
            }
        }
    }

    #[inline]
    pub fn base(&self) -> u64 {
        self.base
    }

    #[inline]
    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }

    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// Mutable access to all of RAM. Drops every cached decoded instruction.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.flush_code();
        &mut self.data
    }

    /// Byte offset of `[paddr, paddr+len)` in RAM, if it lies entirely inside.
    #[inline]
    pub fn offset(&self, paddr: u64, len: u64) -> Option<usize> {
        let off = paddr.wrapping_sub(self.base);
        let size = self.data.len() as u64;
        if off < size && len <= size - off {
            Some(off as usize)
        } else {
            None
        }
    }

    /// Little-endian read at a byte offset that the caller knows is in range.
    #[inline(always)]
    pub(crate) fn read_at(&self, off: usize, size: u8) -> u64 {
        let d = &self.data[off..off + size as usize];
        match size {
            1 => d[0] as u64,
            2 => u16::from_le_bytes([d[0], d[1]]) as u64,
            4 => u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as u64,
            _ => u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]),
        }
    }

    /// Little-endian write at a byte offset that the caller knows is in range.
    #[inline(always)]
    pub(crate) fn write_at(&mut self, off: usize, size: u8, value: u64) {
        self.invalidate(off, size as usize);
        let bytes = value.to_le_bytes();
        self.data[off..off + size as usize].copy_from_slice(&bytes[..size as usize]);
    }

    /// Little-endian read of 1, 2, 4, or 8 bytes. Any alignment.
    #[inline]
    pub fn read(&self, paddr: u64, size: u8) -> Option<u64> {
        let off = self.offset(paddr, size as u64)?;
        let d = &self.data[off..];
        Some(match size {
            1 => d[0] as u64,
            2 => u16::from_le_bytes([d[0], d[1]]) as u64,
            4 => u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as u64,
            8 => u64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]),
            _ => return None,
        })
    }

    /// Little-endian write of 1, 2, 4, or 8 bytes. Any alignment.
    #[inline]
    pub fn write(&mut self, paddr: u64, size: u8, value: u64) -> Option<()> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return None;
        }
        let off = self.offset(paddr, size as u64)?;
        self.write_at(off, size, value);
        Some(())
    }
}

struct Region {
    base: u64,
    size: u64,
    dev: Box<dyn Device>,
}

/// RAM plus a list of device regions. RAM is checked first; device lookup is a linear scan.
pub struct Bus {
    pub ram: Ram,
    regions: Vec<Region>,
    pub wires: Wires,
}

impl Bus {
    pub fn new(ram_base: u64, ram_size: usize) -> Self {
        Bus { ram: Ram::new(ram_base, ram_size), regions: Vec::new(), wires: Wires::default() }
    }

    /// Map `dev` at `[base, base+size)`. Panics if the range overlaps RAM or another device.
    pub fn add_device(&mut self, base: u64, size: u64, dev: Box<dyn Device>) -> DeviceId {
        let end = base.checked_add(size).expect("device range overflows");
        let overlaps = |b: u64, s: u64| base < b + s && b < end;
        assert!(!overlaps(self.ram.base, self.ram.size()), "device at {base:#x} overlaps RAM");
        for r in &self.regions {
            assert!(!overlaps(r.base, r.size), "device at {base:#x} overlaps device at {:#x}", r.base);
        }
        self.regions.push(Region { base, size, dev });
        DeviceId(self.regions.len() - 1)
    }

    pub fn device<T: Device>(&self, id: DeviceId) -> Option<&T> {
        let d: &dyn Any = &*self.regions.get(id.0)?.dev;
        d.downcast_ref::<T>()
    }

    pub fn device_mut<T: Device>(&mut self, id: DeviceId) -> Option<&mut T> {
        let d: &mut dyn Any = &mut *self.regions.get_mut(id.0)?.dev;
        d.downcast_mut::<T>()
    }

    /// The first mapped device of type `T`.
    pub fn find_device_mut<T: Device>(&mut self) -> Option<&mut T> {
        self.regions.iter_mut().find_map(|r| {
            let d: &mut dyn Any = &mut *r.dev;
            d.downcast_mut::<T>()
        })
    }

    /// True if `[paddr, paddr+len)` is entirely RAM.
    #[inline]
    pub fn is_ram(&self, paddr: u64, len: u64) -> bool {
        self.ram.offset(paddr, len).is_some()
    }

    /// True if any byte at `paddr` is backed by RAM or a device.
    pub fn is_mapped(&self, paddr: u64) -> bool {
        self.is_ram(paddr, 1) || self.regions.iter().any(|r| paddr.wrapping_sub(r.base) < r.size)
    }

    /// Physical read. RAM accepts any alignment; devices require natural alignment.
    #[inline]
    pub fn read(&mut self, paddr: u64, size: u8) -> Result<u64, BusFault> {
        if let Some(v) = self.ram.read(paddr, size) {
            return Ok(v);
        }
        self.read_mmio(paddr, size)
    }

    /// Physical write. RAM accepts any alignment; devices require natural alignment.
    #[inline]
    pub fn write(&mut self, paddr: u64, size: u8, value: u64) -> Result<(), BusFault> {
        if self.ram.write(paddr, size, value).is_some() {
            return Ok(());
        }
        self.write_mmio(paddr, size, value)
    }

    #[cold]
    fn read_mmio(&mut self, paddr: u64, size: u8) -> Result<u64, BusFault> {
        let (r, off) = self.region_for(paddr, size)?;
        let r = &mut self.regions[r];
        r.dev.read(off, size, &mut self.wires)
    }

    #[cold]
    fn write_mmio(&mut self, paddr: u64, size: u8, value: u64) -> Result<(), BusFault> {
        let (r, off) = self.region_for(paddr, size)?;
        let r = &mut self.regions[r];
        r.dev.write(off, size, value, &mut self.wires)
    }

    fn region_for(&self, paddr: u64, size: u8) -> Result<(usize, u64), BusFault> {
        if !matches!(size, 1 | 2 | 4 | 8) || paddr & (size as u64 - 1) != 0 {
            return Err(BusFault);
        }
        for (i, r) in self.regions.iter().enumerate() {
            let off = paddr.wrapping_sub(r.base);
            if off < r.size && size as u64 <= r.size - off {
                return Ok((i, off));
            }
        }
        Err(BusFault)
    }

    /// Copy bytes into RAM (image loading). Fails unless the whole range is RAM.
    pub fn write_bytes(&mut self, paddr: u64, bytes: &[u8]) -> Result<(), BusFault> {
        let off = self.ram.offset(paddr, bytes.len() as u64).ok_or(BusFault)?;
        if !bytes.is_empty() {
            self.ram.invalidate(off, bytes.len());
            let first = off >> 12;
            let last = (off + bytes.len() - 1) >> 12;
            for p in first + 1..last {
                if self.ram.code[p].is_some() {
                    self.ram.drop_code(p);
                }
            }
        }
        self.ram.data[off..off + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// Copy bytes out of RAM. Fails unless the whole range is RAM.
    pub fn read_bytes(&self, paddr: u64, buf: &mut [u8]) -> Result<(), BusFault> {
        let off = self.ram.offset(paddr, buf.len() as u64).ok_or(BusFault)?;
        buf.copy_from_slice(&self.ram.data[off..off + buf.len()]);
        Ok(())
    }

    /// Tick every device once.
    pub fn tick(&mut self) {
        for r in &mut self.regions {
            r.dev.tick(&mut self.wires);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(u64);
    impl Device for Scratch {
        fn read(&mut self, _off: u64, _size: u8, _w: &mut Wires) -> Result<u64, BusFault> {
            Ok(self.0)
        }
        fn write(&mut self, _off: u64, _size: u8, v: u64, w: &mut Wires) -> Result<(), BusFault> {
            self.0 = v;
            w.set_irq_source(10, v != 0);
            Ok(())
        }
    }

    #[test]
    fn ram_and_mmio_dispatch() {
        let mut bus = Bus::new(0x8000_0000, 4096);
        let id = bus.add_device(0x1000_0000, 0x100, Box::new(Scratch(0)));
        bus.write(0x8000_0001, 4, 0xdead_beef).unwrap();
        assert_eq!(bus.read(0x8000_0001, 4), Ok(0xdead_beef));
        assert_eq!(bus.read(0x8000_0002, 1), Ok(0xbe));
        assert_eq!(bus.read(0x8000_0ffd, 4), Err(BusFault));
        bus.write(0x1000_0008, 8, 7).unwrap();
        assert!(bus.wires.irq_source(10));
        assert_eq!(bus.read(0x1000_0010, 8), Ok(7));
        assert_eq!(bus.read(0x1000_0011, 2), Err(BusFault));
        assert_eq!(bus.read(0x2000_0000, 4), Err(BusFault));
        assert_eq!(bus.device_mut::<Scratch>(id).map(|s| s.0), Some(7));
    }
}
