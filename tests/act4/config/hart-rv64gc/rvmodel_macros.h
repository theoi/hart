# rvmodel_macros.h
# DUT-specific macro definitions for hart (test profile: hart --elf).
# Derived from riscv-arch-test config/sail/sail-RVA20S64/rvmodel_macros.h (BSD-3-Clause,
# Jordan Carlin and Sadhvi Narayana). The same macros are compiled into the Sail run that
# computes the expected results and into the ELFs that run on hart.
# SPDX-License-Identifier: BSD-3-Clause

#ifndef _RVMODEL_MACROS_H
#define _RVMODEL_MACROS_H

#define CLINT_BASE_ADDRESS 0x02000000

#define RVMODEL_DATA_SECTION \
        .pushsection .tohost,"aw",@progbits;                \
        .balign 8; .global tohost; tohost: .dword 0;         \
        .balign 8; .global fromhost; fromhost: .dword 0;     \
        .popsection

#define STANDARD_SM_SUPPORTED

##### STARTUP #####

// No DUT-specific boot code: hart starts in M-mode at the ELF entry point.
// #define RVMODEL_BOOT
//#define RVMODEL_BOOT_TO_MMODE

##### TERMINATION #####

// HTIF (Host-Target Interface). hart polls tohost between instruction batches and clears
// it after handling a command, so every command is a single 64-bit store (never two
// 32-bit halves), and each store waits until the previous command has been consumed.
// Device 0 command 0 payload 1 = pass (exit 0), payload 3 = fail (exit 1).

# Terminate test with a pass indication.
#define RVMODEL_HALT_PASS  \
  la t0, tohost           ;\
  wait_tohost_pass:       ;\
    ld x1, 0(t0)          ;\
    bnez x1, wait_tohost_pass ;\
  li x1, 1                ;\
  write_tohost_pass:      ;\
    sd x1, 0(t0)          ;\
    j write_tohost_pass   ;\


# Terminate test with a fail indication.
#define RVMODEL_HALT_FAIL \
  la t0, tohost           ;\
  wait_tohost_fail:       ;\
    ld x1, 0(t0)          ;\
    bnez x1, wait_tohost_fail ;\
  li x1, 3                ;\
  write_tohost_fail:      ;\
    sd x1, 0(t0)          ;\
    j write_tohost_fail   ;\


##### IO #####

// No console initialization is needed.
// #define RVMODEL_IO_INIT(_R1, _R2, _R3)

# Prints a null-terminated string through HTIF device 1 (console), command 1 (write):
# tohost = (0x0101 << 48) | byte, written with one 64-bit store after tohost reads 0.
# _R1, _R2, and _R3 are temporaries; _STR_PTR is advanced past the string.
#define RVMODEL_IO_WRITE_STR(_R1, _R2, _R3, _STR_PTR)               \
1:                           ;                       \
  lbu _R1, 0(_STR_PTR)        ;/* Load byte */        \
  beqz _R1, 3f                ;/* Exit if null */     \
  la _R2, tohost              ;                       \
2: /* wait until the previous command is consumed */ ;\
  ld _R3, 0(_R2)              ;                       \
  bnez _R3, 2b                ;                       \
  li _R3, 0x0101000000000000  ;/* device 1, command 1 */ \
  or _R1, _R1, _R3            ;                       \
  sd _R1, 0(_R2)              ;                       \
  addi _STR_PTR, _STR_PTR, 1  ;/* Next char */        \
  j 1b                        ;/* Loop */             \
3:

##### Access Fault #####

// Address 0 is unmapped on hart (and in sail.json): any access raises an access fault.
#define RVMODEL_ACCESS_FAULT_ADDRESS 0x00000000

##### Machine Timer #####

#define RVMODEL_MTIMECMP_ADDRESS  0x02004000  /* CLINT mtimecmp */

#define RVMODEL_MTIME_ADDRESS  0x0200BFF8  /* CLINT mtime */

##### Machine Interrupts #####

// The ELFs run on hart with --timebase insns:1 (mtime advances one tick per retired
// instruction), matching sail.json instructions_per_tick = 1. Sail increments mcycle once
// per platform tick, so instructions_per_tick must be 1 to model hart's mcycle (one per
// retired instruction).
#define RVMODEL_MAX_CYCLES_PER_TIMER_TICK 1

// hart samples interrupts before every instruction, and SIG/CLINT writes reach mip
// immediately. The latency only bounds the idle and mip-polling loops.
#define RVMODEL_INTERRUPT_LATENCY 10

// mtimecmp = mtime + 20000 ticks (20,000 instructions). hart updates mtime, and so MTIP,
// between batches of at most 10,000 instructions (BATCH in crates/hart-core/src/board.rs),
// so the idle loop (20,000 iterations, about 40,000 instructions) covers the delay plus one
// batch.
#define RVMODEL_TIMER_INT_SOON_DELAY 20000

// SIG: Sail-compatible simple interrupt generator at 0x0C000000 (hart test profile).
// PLATFORM register at +4: bit 31 = level, bits 11/9/3/1 select MEI/SEI/MSI/SSI.
#define SIG_ADDRESS  (0xC000000 + 0x4)
#define RVMODEL_SET_MEXT_INT(_R1, _R2)        \
  li _R1, (1 << 31) | (1 << 11);               \
  li _R2, SIG_ADDRESS;    \
  sw _R1, 0(_R2)            ; /* Set MEXT interrupt */ \


#define RVMODEL_CLR_MEXT_INT(_R1, _R2)        \
  li _R1, (1 << 11);               \
  li _R2, SIG_ADDRESS;    \
  sw _R1, 0(_R2)            ; /* Clear MEXT interrupt */ \

#define RVMODEL_MSIP_ADDRESS (CLINT_BASE_ADDRESS + 0x0)

##### Supervisor Interrupts #####

#define RVMODEL_SET_SEXT_INT(_R1, _R2)        \
  li _R1, (1 << 31) | (1 << 9);               \
  li _R2, SIG_ADDRESS;    \
  sw _R1, 0(_R2)            ; /* Set SEXT interrupt */ \

#define RVMODEL_CLR_SEXT_INT(_R1, _R2)        \
  li _R1, (1 << 9);               \
  li _R2, SIG_ADDRESS;    \
  sw _R1, 0(_R2)            ; /* Clear SEXT interrupt */ \


#define RVMODEL_SET_SSW_INT(_R1, _R2)        \
  li _R1, (1 << 31) | (1 << 1);               \
  li _R2, SIG_ADDRESS;    \
  sw _R1, 0(_R2)            ; /* Set SSW interrupt */ \

#define RVMODEL_CLR_SSW_INT(_R1, _R2)        \
  li _R1, (1 << 1);               \
  li _R2, SIG_ADDRESS;    \
  sw _R1, 0(_R2)            ; /* Clear SSW interrupt */ \

// No Sscofpmf: the RVMODEL_HPM_COUNTER / RVMODEL_MHPMEVENT_* macros are not needed.

#endif // _RVMODEL_MACROS_H
