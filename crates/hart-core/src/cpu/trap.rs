//! Exceptions, interrupts, and traps.

/// Synchronous exception causes (`mcause` with the interrupt bit clear).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ExceptionCause {
    InstructionAddressMisaligned = 0,
    InstructionAccessFault = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadAddressMisaligned = 4,
    LoadAccessFault = 5,
    StoreAddressMisaligned = 6,
    StoreAccessFault = 7,
    EcallFromU = 8,
    EcallFromS = 9,
    EcallFromM = 11,
    InstructionPageFault = 12,
    LoadPageFault = 13,
    StorePageFault = 15,
}

impl ExceptionCause {
    pub fn name(self) -> &'static str {
        use ExceptionCause::*;
        match self {
            InstructionAddressMisaligned => "trap_instruction_address_misaligned",
            InstructionAccessFault => "trap_instruction_access_fault",
            IllegalInstruction => "trap_illegal_instruction",
            Breakpoint => "trap_breakpoint",
            LoadAddressMisaligned => "trap_load_address_misaligned",
            LoadAccessFault => "trap_load_access_fault",
            StoreAddressMisaligned => "trap_store_address_misaligned",
            StoreAccessFault => "trap_store_access_fault",
            EcallFromU => "trap_user_ecall",
            EcallFromS => "trap_supervisor_ecall",
            EcallFromM => "trap_machine_ecall",
            InstructionPageFault => "trap_instruction_page_fault",
            LoadPageFault => "trap_load_page_fault",
            StorePageFault => "trap_store_page_fault",
        }
    }
}

/// A synchronous exception with the value hart writes to `xtval`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Exception {
    pub cause: ExceptionCause,
    pub tval: u64,
}

impl Exception {
    #[inline]
    pub const fn new(cause: ExceptionCause, tval: u64) -> Self {
        Exception { cause, tval }
    }

    /// Illegal instruction; `xtval` gets the instruction bits (16 bits for compressed).
    #[inline]
    pub const fn illegal(insn: u32) -> Self {
        Exception::new(ExceptionCause::IllegalInstruction, insn as u64)
    }

    #[inline]
    pub fn code(&self) -> u64 {
        self.cause as u64
    }
}

/// Interrupt causes (`mcause` with the interrupt bit set). The value is the cause code,
/// which is also the bit position in `mip`/`mie`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Interrupt {
    SupervisorSoftware = 1,
    MachineSoftware = 3,
    SupervisorTimer = 5,
    MachineTimer = 7,
    SupervisorExternal = 9,
    MachineExternal = 11,
}

impl Interrupt {
    #[inline]
    pub fn code(self) -> u64 {
        self as u64
    }

    #[inline]
    pub fn mask(self) -> u64 {
        1 << (self as u64)
    }

    pub fn from_code(code: u64) -> Option<Interrupt> {
        use Interrupt::*;
        Some(match code {
            1 => SupervisorSoftware,
            3 => MachineSoftware,
            5 => SupervisorTimer,
            7 => MachineTimer,
            9 => SupervisorExternal,
            11 => MachineExternal,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trap {
    Exception(Exception),
    Interrupt(Interrupt),
}

impl Trap {
    /// The value written to `xcause`.
    pub fn cause(&self) -> u64 {
        match self {
            Trap::Exception(e) => e.code(),
            Trap::Interrupt(i) => (1 << 63) | i.code(),
        }
    }

    pub fn tval(&self) -> u64 {
        match self {
            Trap::Exception(e) => e.tval,
            Trap::Interrupt(_) => 0,
        }
    }
}

impl From<Exception> for Trap {
    fn from(e: Exception) -> Self {
        Trap::Exception(e)
    }
}
