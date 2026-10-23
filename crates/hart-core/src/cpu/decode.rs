//! Instruction decoding, including expansion of all compressed (C) encodings.

/// Length in bytes of the instruction whose low 16 bits are `lo`.
#[inline]
pub fn insn_len(lo: u16) -> u8 {
    if lo & 3 == 3 {
        4
    } else {
        2
    }
}
