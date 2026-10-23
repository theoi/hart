//! End-to-end tests of the `hart` binary: device tree dump, bare-metal test
//! exit status, and the instruction limit.

use hart_core::fdt::Fdt;
use std::path::PathBuf;
use std::process::Command;

fn hart() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hart"))
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hart-cli-test-{}-{name}", std::process::id()))
}

/// A minimal RISC-V ELF: `code` at 0x8000_0000 and a `tohost` symbol at
/// 0x8000_1000.
fn elf(code: &[u32]) -> Vec<u8> {
    let code: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let strtab = b"\0tohost\0";
    let mut symtab = vec![0u8; 48];
    symtab[24..28].copy_from_slice(&1u32.to_le_bytes());
    symtab[32..40].copy_from_slice(&0x8000_1000u64.to_le_bytes());
    let code_off = 64 + 56;
    let sym_off = code_off + code.len();
    let str_off = sym_off + symtab.len();
    let sh_off = (str_off + strtab.len() + 7) & !7;
    let mut b = vec![0u8; sh_off + 3 * 64];
    let put = |b: &mut Vec<u8>, at: usize, bytes: &[u8]| b[at..at + bytes.len()].copy_from_slice(bytes);
    put(&mut b, 0, b"\x7fELF\x02\x01\x01");
    put(&mut b, 16, &2u16.to_le_bytes());
    put(&mut b, 18, &243u16.to_le_bytes());
    put(&mut b, 24, &0x8000_0000u64.to_le_bytes());
    put(&mut b, 32, &64u64.to_le_bytes());
    put(&mut b, 40, &(sh_off as u64).to_le_bytes());
    put(&mut b, 54, &56u16.to_le_bytes());
    put(&mut b, 56, &1u16.to_le_bytes());
    put(&mut b, 58, &64u16.to_le_bytes());
    put(&mut b, 60, &3u16.to_le_bytes());
    put(&mut b, 64, &1u32.to_le_bytes());
    put(&mut b, 72, &(code_off as u64).to_le_bytes());
    put(&mut b, 80, &0x8000_0000u64.to_le_bytes());
    put(&mut b, 88, &0x8000_0000u64.to_le_bytes());
    put(&mut b, 96, &(code.len() as u64).to_le_bytes());
    put(&mut b, 104, &0x2000u64.to_le_bytes());
    put(&mut b, code_off, &code);
    put(&mut b, sym_off, &symtab);
    put(&mut b, str_off, strtab);
    let sh1 = sh_off + 64;
    put(&mut b, sh1 + 4, &2u32.to_le_bytes());
    put(&mut b, sh1 + 24, &(sym_off as u64).to_le_bytes());
    put(&mut b, sh1 + 32, &(symtab.len() as u64).to_le_bytes());
    put(&mut b, sh1 + 40, &2u32.to_le_bytes());
    let sh2 = sh_off + 128;
    put(&mut b, sh2 + 4, &3u32.to_le_bytes());
    put(&mut b, sh2 + 24, &(str_off as u64).to_le_bytes());
    put(&mut b, sh2 + 32, &(strtab.len() as u64).to_le_bytes());
    b
}

/// auipc t0, 1; li t1, value; sd t1, 0(t0); j .
fn htif_exit(value: u32) -> Vec<u32> {
    vec![0x0000_1297, (value << 20) | 0x0000_0313, 0x0062_b023, 0x0000_006f]
}

fn run_elf(name: &str, code: &[u32], extra: &[&str]) -> std::process::Output {
    let path = temp_path(name);
    std::fs::write(&path, elf(code)).unwrap();
    let out = hart().arg("--elf").arg(&path).args(extra).output().unwrap();
    let _ = std::fs::remove_file(&path);
    out
}

#[test]
fn elf_pass_exits_zero() {
    let out = run_elf("pass.elf", &htif_exit(1), &["-m", "1"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn elf_failure_exits_one_and_reports_the_code() {
    let out = run_elf("fail.elf", &htif_exit((5 << 1) | 1), &["-m", "1"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("code 5"));
}

#[test]
fn instruction_limit_exits_124() {
    let out = run_elf("loop.elf", &[0x0000_006f], &["-m", "1", "--max-insns", "20000"]);
    assert_eq!(out.status.code(), Some(124));
}

#[test]
fn dump_dtb_describes_the_ram_size() {
    let path = temp_path("virt.dtb");
    let status =
        hart().args(["-m", "512", "--append", "console=ttyS0 quiet", "--dump-dtb"]).arg(&path).status().unwrap();
    assert!(status.success());
    let fdt = Fdt::parse(&std::fs::read(&path).unwrap()).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        fdt.root.find("/memory@80000000").unwrap().property_cells("reg"),
        Some(vec![0, 0x8000_0000, 0, 0x2000_0000])
    );
    assert_eq!(fdt.root.find("/chosen").unwrap().property_string("bootargs"), Some("console=ttyS0 quiet"));
}

#[test]
fn missing_firmware_is_an_error() {
    let out = hart().stdin(std::process::Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no firmware"));
}
