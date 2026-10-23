//! Flattened device tree (DTB) writer, format version 17, plus a small
//! reader used to inspect blobs (tests, `--dtb` sanity checks).

use std::collections::BTreeMap;
use std::fmt;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
const HEADER_SIZE: usize = 40;
const VERSION: u32 = 17;
const LAST_COMPATIBLE_VERSION: u32 = 16;

/// Builds a device tree blob node by node.
///
/// Nodes must be properly nested, and a node's properties must come before
/// its first child node; violations panic, since they are programming errors
/// in the board description.
#[derive(Debug, Default)]
pub struct FdtWriter {
    structure: Vec<u8>,
    strings: Vec<u8>,
    string_offsets: BTreeMap<String, u32>,
    reservations: Vec<(u64, u64)>,
    depth: usize,
    has_root: bool,
    child_open_at_depth: Vec<bool>,
    boot_cpuid: u32,
}

impl FdtWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_boot_cpuid(&mut self, cpuid: u32) {
        self.boot_cpuid = cpuid;
    }

    /// Adds a memory reservation map entry.
    pub fn add_reservation(&mut self, address: u64, size: u64) {
        self.reservations.push((address, size));
    }

    fn push_u32(&mut self, v: u32) {
        self.structure.extend_from_slice(&v.to_be_bytes());
    }

    fn pad(&mut self) {
        while self.structure.len() % 4 != 0 {
            self.structure.push(0);
        }
    }

    /// Opens a node. The root node's name is the empty string.
    pub fn begin_node(&mut self, name: &str) {
        assert!(!name.contains('\0'), "node name contains NUL");
        if self.depth == 0 {
            assert!(!self.has_root, "only one root node is allowed");
            assert!(name.is_empty(), "the root node name must be empty");
            self.has_root = true;
        } else {
            assert!(!name.is_empty(), "only the root node may be unnamed");
            *self.child_open_at_depth.last_mut().unwrap() = true;
        }
        self.push_u32(FDT_BEGIN_NODE);
        self.structure.extend_from_slice(name.as_bytes());
        self.structure.push(0);
        self.pad();
        self.depth += 1;
        self.child_open_at_depth.push(false);
    }

    pub fn end_node(&mut self) {
        assert!(self.depth > 0, "end_node without begin_node");
        self.push_u32(FDT_END_NODE);
        self.depth -= 1;
        self.child_open_at_depth.pop();
    }

    fn string_offset(&mut self, name: &str) -> u32 {
        if let Some(&off) = self.string_offsets.get(name) {
            return off;
        }
        let off = self.strings.len() as u32;
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        self.string_offsets.insert(name.to_string(), off);
        off
    }

    /// Adds a property with a raw value.
    pub fn property(&mut self, name: &str, value: &[u8]) {
        assert!(self.depth > 0, "property outside a node");
        assert!(!self.child_open_at_depth.last().unwrap(), "property {name:?} after a child node");
        assert!(!name.is_empty() && !name.contains('\0'));
        let name_off = self.string_offset(name);
        self.push_u32(FDT_PROP);
        self.push_u32(value.len() as u32);
        self.push_u32(name_off);
        self.structure.extend_from_slice(value);
        self.pad();
    }

    pub fn property_empty(&mut self, name: &str) {
        self.property(name, &[]);
    }

    pub fn property_u32(&mut self, name: &str, value: u32) {
        self.property(name, &value.to_be_bytes());
    }

    pub fn property_u32s(&mut self, name: &str, values: &[u32]) {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        self.property(name, &bytes);
    }

    /// A 64-bit value as two cells.
    pub fn property_u64(&mut self, name: &str, value: u64) {
        self.property(name, &value.to_be_bytes());
    }

    /// 64-bit values, two cells each (for `reg` with 2 address/size cells).
    pub fn property_u64s(&mut self, name: &str, values: &[u64]) {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        self.property(name, &bytes);
    }

    pub fn property_string(&mut self, name: &str, value: &str) {
        self.property_strings(name, &[value]);
    }

    /// A string list, each entry NUL-terminated.
    pub fn property_strings(&mut self, name: &str, values: &[&str]) {
        let mut bytes = Vec::new();
        for v in values {
            assert!(!v.contains('\0'), "string property contains NUL");
            bytes.extend_from_slice(v.as_bytes());
            bytes.push(0);
        }
        self.property(name, &bytes);
    }

    /// Finishes the blob. Panics if nodes are still open or no root exists.
    pub fn finish(mut self) -> Vec<u8> {
        assert!(self.has_root, "device tree has no root node");
        assert_eq!(self.depth, 0, "unclosed device tree nodes");
        self.push_u32(FDT_END);

        let rsvmap_off = HEADER_SIZE;
        let rsvmap_size = 16 * (self.reservations.len() + 1);
        let struct_off = rsvmap_off + rsvmap_size;
        let strings_off = struct_off + self.structure.len();
        let total = strings_off + self.strings.len();

        let mut blob = Vec::with_capacity(total);
        for v in [
            FDT_MAGIC,
            total as u32,
            struct_off as u32,
            strings_off as u32,
            rsvmap_off as u32,
            VERSION,
            LAST_COMPATIBLE_VERSION,
            self.boot_cpuid,
            self.strings.len() as u32,
            self.structure.len() as u32,
        ] {
            blob.extend_from_slice(&v.to_be_bytes());
        }
        for (addr, size) in &self.reservations {
            blob.extend_from_slice(&addr.to_be_bytes());
            blob.extend_from_slice(&size.to_be_bytes());
        }
        blob.extend_from_slice(&[0u8; 16]);
        blob.extend_from_slice(&self.structure);
        blob.extend_from_slice(&self.strings);
        debug_assert_eq!(blob.len(), total);
        blob
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FdtError {
    Truncated,
    BadMagic,
    UnsupportedVersion(u32),
    BadStructure(usize),
}

impl fmt::Display for FdtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "device tree blob is truncated"),
            Self::BadMagic => write!(f, "not a device tree blob"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported device tree version {v}"),
            Self::BadStructure(off) => write!(f, "malformed device tree structure at {off:#x}"),
        }
    }
}

impl std::error::Error for FdtError {}

/// A parsed device tree node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FdtNode {
    pub name: String,
    pub properties: Vec<(String, Vec<u8>)>,
    pub children: Vec<FdtNode>,
}

impl FdtNode {
    pub fn property(&self, name: &str) -> Option<&[u8]> {
        self.properties.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice())
    }

    pub fn child(&self, name: &str) -> Option<&FdtNode> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Looks up a node by absolute path such as `/soc/serial@10000000`.
    pub fn find(&self, path: &str) -> Option<&FdtNode> {
        path.split('/').filter(|s| !s.is_empty()).try_fold(self, |node, part| node.child(part))
    }

    pub fn property_u32(&self, name: &str) -> Option<u32> {
        let v = self.property(name)?;
        (v.len() == 4).then(|| u32::from_be_bytes(v.try_into().unwrap()))
    }

    pub fn property_cells(&self, name: &str) -> Option<Vec<u32>> {
        let v = self.property(name)?;
        (v.len() % 4 == 0).then(|| v.chunks_exact(4).map(|c| u32::from_be_bytes(c.try_into().unwrap())).collect())
    }

    pub fn property_strings(&self, name: &str) -> Option<Vec<&str>> {
        let v = self.property(name)?;
        let v = v.strip_suffix(&[0])?;
        v.split(|&b| b == 0).map(|s| std::str::from_utf8(s).ok()).collect()
    }

    pub fn property_string(&self, name: &str) -> Option<&str> {
        let strings = self.property_strings(name)?;
        (strings.len() == 1).then(|| strings[0])
    }
}

/// A parsed device tree blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fdt {
    pub boot_cpuid: u32,
    pub reservations: Vec<(u64, u64)>,
    pub root: FdtNode,
}

fn be32(b: &[u8], off: usize) -> Result<u32, FdtError> {
    b.get(off..off + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap())).ok_or(FdtError::Truncated)
}

fn be64(b: &[u8], off: usize) -> Result<u64, FdtError> {
    b.get(off..off + 8).map(|s| u64::from_be_bytes(s.try_into().unwrap())).ok_or(FdtError::Truncated)
}

impl Fdt {
    pub fn parse(blob: &[u8]) -> Result<Self, FdtError> {
        if be32(blob, 0)? != FDT_MAGIC {
            return Err(FdtError::BadMagic);
        }
        let total = be32(blob, 4)? as usize;
        if blob.len() < total {
            return Err(FdtError::Truncated);
        }
        let blob = &blob[..total];
        let struct_off = be32(blob, 8)? as usize;
        let strings_off = be32(blob, 12)? as usize;
        let rsvmap_off = be32(blob, 16)? as usize;
        let version = be32(blob, 20)?;
        if version < LAST_COMPATIBLE_VERSION {
            return Err(FdtError::UnsupportedVersion(version));
        }
        let boot_cpuid = be32(blob, 28)?;
        let strings = blob.get(strings_off..).ok_or(FdtError::Truncated)?;

        let mut reservations = Vec::new();
        let mut off = rsvmap_off;
        loop {
            let (addr, size) = (be64(blob, off)?, be64(blob, off + 8)?);
            off += 16;
            if addr == 0 && size == 0 {
                break;
            }
            reservations.push((addr, size));
        }

        let mut stack: Vec<FdtNode> = Vec::new();
        let mut root = None;
        let mut off = struct_off;
        loop {
            let token = be32(blob, off)?;
            let token_off = off;
            off += 4;
            match token {
                FDT_BEGIN_NODE => {
                    let rest = blob.get(off..).ok_or(FdtError::Truncated)?;
                    let len = rest.iter().position(|&b| b == 0).ok_or(FdtError::Truncated)?;
                    let name = String::from_utf8_lossy(&rest[..len]).into_owned();
                    off = (off + len + 1 + 3) & !3;
                    if root.is_some() {
                        return Err(FdtError::BadStructure(token_off));
                    }
                    stack.push(FdtNode { name, ..FdtNode::default() });
                }
                FDT_END_NODE => {
                    let node = stack.pop().ok_or(FdtError::BadStructure(token_off))?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(node),
                        None => root = Some(node),
                    }
                }
                FDT_PROP => {
                    let len = be32(blob, off)? as usize;
                    let name_off = be32(blob, off + 4)? as usize;
                    off += 8;
                    let value = blob.get(off..off + len).ok_or(FdtError::Truncated)?;
                    off = (off + len + 3) & !3;
                    let name_bytes = strings.get(name_off..).ok_or(FdtError::Truncated)?;
                    let name_len = name_bytes.iter().position(|&b| b == 0).ok_or(FdtError::Truncated)?;
                    let name = String::from_utf8_lossy(&name_bytes[..name_len]).into_owned();
                    stack.last_mut().ok_or(FdtError::BadStructure(token_off))?.properties.push((name, value.to_vec()));
                }
                FDT_NOP => {}
                FDT_END => break,
                _ => return Err(FdtError::BadStructure(token_off)),
            }
        }
        if !stack.is_empty() {
            return Err(FdtError::BadStructure(off));
        }
        Ok(Self { boot_cpuid, reservations, root: root.ok_or(FdtError::BadStructure(struct_off))? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut w = FdtWriter::new();
        w.add_reservation(0x8000_0000, 0x4_0000);
        w.begin_node("");
        w.property_u32("#address-cells", 2);
        w.property_u32("#size-cells", 2);
        w.property_strings("compatible", &["hart,virt", "riscv-virtio"]);
        w.begin_node("chosen");
        w.property_string("bootargs", "console=ttyS0");
        w.end_node();
        w.begin_node("memory@80000000");
        w.property_string("device_type", "memory");
        w.property_u64s("reg", &[0x8000_0000, 0x1000_0000]);
        w.end_node();
        w.begin_node("intc");
        w.property_empty("interrupt-controller");
        w.property_u32s("interrupts-extended", &[1, 11, 1, 9]);
        w.property_u32("#size-cells", 0);
        w.end_node();
        w.end_node();
        w.finish()
    }

    #[test]
    fn header_layout() {
        let blob = sample();
        assert_eq!(&blob[0..4], &[0xd0, 0x0d, 0xfe, 0xed]);
        assert_eq!(be32(&blob, 4).unwrap() as usize, blob.len());
        assert_eq!(be32(&blob, 16).unwrap(), 40, "rsvmap after header");
        assert_eq!(be32(&blob, 8).unwrap(), 40 + 32, "struct after rsvmap");
        assert_eq!(be32(&blob, 20).unwrap(), 17);
        assert_eq!(be32(&blob, 24).unwrap(), 16);
        let struct_off = be32(&blob, 8).unwrap() as usize;
        let struct_size = be32(&blob, 36).unwrap() as usize;
        let strings_off = be32(&blob, 12).unwrap() as usize;
        assert_eq!(struct_off + struct_size, strings_off);
        assert_eq!(be32(&blob, strings_off - 4).unwrap(), FDT_END);
        assert_eq!(struct_off % 8, 0);
    }

    #[test]
    fn strings_are_deduplicated() {
        let blob = sample();
        let strings_off = be32(&blob, 12).unwrap() as usize;
        let strings = &blob[strings_off..];
        let count = strings.split(|&b| b == 0).filter(|s| *s == b"#size-cells").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn round_trip() {
        let fdt = Fdt::parse(&sample()).unwrap();
        assert_eq!(fdt.reservations, vec![(0x8000_0000, 0x4_0000)]);
        let root = &fdt.root;
        assert_eq!(root.name, "");
        assert_eq!(root.property_u32("#address-cells"), Some(2));
        assert_eq!(root.property_strings("compatible"), Some(vec!["hart,virt", "riscv-virtio"]));
        assert_eq!(root.find("/chosen").unwrap().property_string("bootargs"), Some("console=ttyS0"));
        let mem = root.find("/memory@80000000").unwrap();
        assert_eq!(mem.property_cells("reg"), Some(vec![0, 0x8000_0000, 0, 0x1000_0000]));
        let intc = root.find("intc").unwrap();
        assert_eq!(intc.property("interrupt-controller"), Some(&[][..]));
        assert_eq!(intc.property_cells("interrupts-extended"), Some(vec![1, 11, 1, 9]));
        assert!(root.find("/nope").is_none());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(Fdt::parse(&[0; 8]).unwrap_err(), FdtError::BadMagic);
        let blob = sample();
        assert_eq!(Fdt::parse(&blob[..20]).unwrap_err(), FdtError::Truncated);
    }

    #[test]
    #[should_panic(expected = "after a child node")]
    fn property_after_child_panics() {
        let mut w = FdtWriter::new();
        w.begin_node("");
        w.begin_node("a");
        w.end_node();
        w.property_u32("x", 1);
    }

    #[test]
    #[should_panic(expected = "unclosed")]
    fn unclosed_node_panics() {
        let mut w = FdtWriter::new();
        w.begin_node("");
        w.finish();
    }
}
