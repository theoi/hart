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
/// access is naturally aligned unless [`Device::any_alignment`] says otherwise:
/// misaligned accesses to other devices raise access faults before reaching the device.
/// An access never extends past the end of the device's region. Read values are returned
/// zero-extended in the low `size` bytes; written values carry the data in the low `size`
/// bytes.
pub trait Device: Any {
    fn read(&mut self, offset: u64, size: u8, wires: &mut Wires) -> Result<u64, BusFault>;
    fn write(&mut self, offset: u64, size: u8, value: u64, wires: &mut Wires) -> Result<(), BusFault>;
    /// Whether the device takes accesses at any alignment, as memory does (a ROM).
    fn any_alignment(&self) -> bool {
        false
    }
    /// Whether instructions may be fetched from the device (a ROM). A fetch from any
    /// other device raises an instruction access fault.
    fn executable(&self) -> bool {
        false
    }
    /// Called by the board's run loop between instruction batches, after `wires.mtime`
    /// has been advanced. Devices update interrupt lines here.
    fn tick(&mut self, _wires: &mut Wires) {}
}

/// Handle returned by [`Bus::add_device`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(usize);

/// Decoded-instruction slots per 4 KiB page, one per 2-byte-aligned address.
pub const CODE_SLOTS: usize = 2048;
const CODE_SHIFT: u32 = 11;

/// Main memory, plus a cache of decoded instructions for each RAM page that has been
/// executed. Every write into a page drops that page's decoded instructions, so the
/// cache never holds stale code.
pub struct Ram {
    base: u64,
    data: Vec<u8>,
    /// For each RAM page, the index of its decoded page in `code`, or 0 if it has none.
    code_page: Vec<u32>,
    /// Decoded instructions, `CODE_SLOTS` per decoded page. Decoded page 0 is never
    /// assigned and stays all `Uncached`, so a RAM page without decoded instructions
    /// reads as uncached with no separate check.
    code: Vec<Insn>,
    /// Decoded pages free for reuse.
    free: Vec<u32>,
    /// Set when a write through [`Ram::write`] or [`Bus::write`] drops decoded instructions.
    code_dropped: bool,
    /// Set when a RAM page is given decoded instructions.
    code_page_added: bool,
}

impl Ram {
    pub fn new(base: u64, size: usize) -> Self {
        let pages = size.div_ceil(4096);
        Ram {
            base,
            data: vec![0; size],
            code_page: vec![0; pages],
            code: vec![Insn::UNCACHED; CODE_SLOTS],
            free: Vec::new(),
            code_dropped: false,
            code_page_added: false,
        }
    }

    /// Index in the decoded-instruction cache of the slot for byte offset `off`. The slot
    /// holds `Op::Uncached` until [`Ram::cache_code`] fills it.
    #[inline(always)]
    pub(crate) fn code_slot(&self, off: usize) -> usize {
        ((self.code_page[off >> 12] as usize) << CODE_SHIFT) | ((off & 0xfff) >> 1)
    }

    /// Index of the first slot of the decoded page for the RAM page holding `off`,
    /// assigning an empty decoded page if it has none. The index stays valid until a
    /// write into that RAM page (see [`Ram::write_at`]).
    #[inline]
    pub(crate) fn code_page_base(&mut self, off: usize) -> usize {
        let p = off >> 12;
        let mut idx = self.code_page[p];
        if idx == 0 {
            idx = self.assign_code_page(p);
        }
        (idx as usize) << CODE_SHIFT
    }

    /// The decoded instruction in cache slot `slot`.
    #[inline(always)]
    pub(crate) fn code(&self, slot: usize) -> Insn {
        self.code[slot]
    }

    /// Cache the decode of the instruction at byte offset `off`. The instruction must
    /// lie entirely inside one page.
    pub(crate) fn cache_code(&mut self, off: usize, insn: Insn) {
        let base = self.code_page_base(off);
        self.code[base | ((off & 0xfff) >> 1)] = insn;
    }

    #[cold]
    fn assign_code_page(&mut self, p: usize) -> u32 {
        let idx = match self.free.pop() {
            Some(idx) => {
                let b = (idx as usize) << CODE_SHIFT;
                self.code[b..b + CODE_SLOTS].fill(Insn::UNCACHED);
                idx
            }
            None => {
                let idx = (self.code.len() >> CODE_SHIFT) as u32;
                self.code.resize(self.code.len() + CODE_SLOTS, Insn::UNCACHED);
                idx
            }
        };
        self.code_page[p] = idx;
        self.code_page_added = true;
        idx
    }

    /// Whether the RAM page holding byte offset `off` has decoded instructions.
    #[inline]
    pub(crate) fn has_code(&self, off: usize) -> bool {
        self.code_page[off >> 12] != 0
    }

    /// Whether a RAM page was given decoded instructions since the last call. Clears
    /// the flag.
    #[inline]
    pub(crate) fn take_code_page_added(&mut self) -> bool {
        core::mem::take(&mut self.code_page_added)
    }

    /// Drop decoded instructions for the pages touched by a write of `len` bytes at
    /// `off`. Returns true if any were dropped.
    #[inline(always)]
    fn invalidate(&mut self, off: usize, len: usize) -> bool {
        let first = off >> 12;
        let last = (off + len - 1) >> 12;
        let mut dropped = false;
        if self.code_page[first] != 0 {
            self.drop_code(first);
            dropped = true;
        }
        if last != first && self.code_page[last] != 0 {
            self.drop_code(last);
            dropped = true;
        }
        dropped
    }

    #[cold]
    fn drop_code(&mut self, page: usize) {
        self.free.push(self.code_page[page]);
        self.code_page[page] = 0;
    }

    /// Drop all decoded instructions.
    pub fn flush_code(&mut self) {
        self.code_page.fill(0);
        self.code.truncate(CODE_SLOTS);
        self.free.clear();
        self.code_dropped = true;
    }

    /// Whether a write through [`Ram::write`] dropped decoded instructions since the
    /// last call. Clears the flag.
    #[inline]
    pub(crate) fn take_code_dropped(&mut self) -> bool {
        core::mem::take(&mut self.code_dropped)
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

    /// Little-endian write at a byte offset that the caller knows is in range. Returns
    /// true if the write dropped decoded instructions, which invalidates indices from
    /// [`Ram::code_page_base`].
    #[inline(always)]
    fn write_at(&mut self, off: usize, size: u8, value: u64) -> bool {
        let dropped = self.invalidate(off, size as usize);
        self.write_no_code(off, size, value);
        dropped
    }

    /// Little-endian write at a byte offset that the caller knows is in range, inside
    /// one page that holds no decoded instructions (as a store TLB hit guarantees).
    #[inline(always)]
    pub(crate) fn write_no_code(&mut self, off: usize, size: u8, value: u64) {
        debug_assert!(!self.has_code(off) && !self.has_code(off + size as usize - 1));
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
        if self.write_at(off, size, value) {
            self.code_dropped = true;
        }
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

    /// True if instructions can be fetched from `[paddr, paddr+len)`: it is RAM, or lies
    /// in one device marked [`Device::executable`].
    pub fn is_executable(&self, paddr: u64, len: u64) -> bool {
        self.is_ram(paddr, len)
            || self.regions.iter().any(|r| {
                let off = paddr.wrapping_sub(r.base);
                off < r.size && len <= r.size - off && r.dev.executable()
            })
    }

    /// True if any byte at `paddr` is backed by RAM or a device.
    pub fn is_mapped(&self, paddr: u64) -> bool {
        self.is_ram(paddr, 1) || self.regions.iter().any(|r| paddr.wrapping_sub(r.base) < r.size)
    }

    /// Physical read. RAM accepts any alignment; devices require natural alignment unless
    /// they accept any ([`Device::any_alignment`]).
    #[inline]
    pub fn read(&mut self, paddr: u64, size: u8) -> Result<u64, BusFault> {
        if let Some(v) = self.ram.read(paddr, size) {
            return Ok(v);
        }
        self.read_mmio(paddr, size)
    }

    /// Physical write. RAM accepts any alignment; devices require natural alignment unless
    /// they accept any ([`Device::any_alignment`]).
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
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err(BusFault);
        }
        let aligned = paddr & (size as u64 - 1) == 0;
        for (i, r) in self.regions.iter().enumerate() {
            let off = paddr.wrapping_sub(r.base);
            if off < r.size && size as u64 <= r.size - off && (aligned || r.dev.any_alignment()) {
                return Ok((i, off));
            }
        }
        Err(BusFault)
    }

    /// Copy bytes into RAM (image loading). Fails unless the whole range is RAM.
    pub fn write_bytes(&mut self, paddr: u64, bytes: &[u8]) -> Result<(), BusFault> {
        let off = self.ram.offset(paddr, bytes.len() as u64).ok_or(BusFault)?;
        if !bytes.is_empty() {
            for p in off >> 12..=(off + bytes.len() - 1) >> 12 {
                if self.ram.code_page[p] != 0 {
                    self.ram.drop_code(p);
                    self.ram.code_dropped = true;
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

    struct Rom([u8; 16]);
    impl Device for Rom {
        fn read(&mut self, off: u64, size: u8, _w: &mut Wires) -> Result<u64, BusFault> {
            let mut b = [0u8; 8];
            b[..size as usize].copy_from_slice(&self.0[off as usize..off as usize + size as usize]);
            Ok(u64::from_le_bytes(b))
        }
        fn write(&mut self, _off: u64, _size: u8, _v: u64, _w: &mut Wires) -> Result<(), BusFault> {
            Err(BusFault)
        }
        fn any_alignment(&self) -> bool {
            true
        }
    }

    #[test]
    fn any_alignment_devices_take_misaligned_accesses_within_their_region() {
        let mut bus = Bus::new(0x8000_0000, 4096);
        bus.add_device(0x1000, 16, Box::new(Rom(core::array::from_fn(|i| i as u8))));
        bus.add_device(0x1000_0000, 0x100, Box::new(Scratch(0)));
        assert_eq!(bus.read(0x1003, 8), Ok(0x0a09_0807_0605_0403));
        assert_eq!(bus.read(0x1009, 8), Err(BusFault), "past the end of the region");
        assert_eq!(bus.write(0x1003, 4, 0), Err(BusFault));
        assert_eq!(bus.read(0x1000_0004, 8), Err(BusFault), "other devices still need alignment");
    }
}
