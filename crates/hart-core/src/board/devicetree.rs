//! Device tree for the hart virt board, compatible with QEMU's virt memory
//! map so that unmodified OpenSBI (generic platform) and Linux find their
//! devices.

use crate::devices::clint::{CLINT_BASE, CLINT_SIZE};
use crate::devices::plic::{PLIC_BASE, PLIC_NDEV, PLIC_SIZE};
use crate::devices::syscon::{FINISHER_PASS, FINISHER_RESET, SYSCON_BASE, SYSCON_SIZE};
use crate::devices::uart::{UART_BASE, UART_CLOCK_HZ, UART_IRQ, UART_SIZE};
use crate::fdt::FdtWriter;

/// mtime frequency, as on QEMU virt and Spike.
pub const TIMEBASE_HZ: u32 = 10_000_000;

/// The ISA string in `riscv,isa`. Zaamo/Zalrsc and Zca/Zcd are implied by
/// A and C (+D), and Linux reports them on its own.
pub const ISA_STRING: &str = "rv64imafdc_zicntr_zicsr_zifencei_zihpm";
/// The same ISA in the newer `riscv,isa-extensions` form.
pub const ISA_EXTENSIONS: &[&str] = &["i", "m", "a", "f", "d", "c", "zicntr", "zicsr", "zifencei", "zihpm"];

const PHANDLE_CPU0: u32 = 1;
const PHANDLE_CPU0_INTC: u32 = 2;
const PHANDLE_PLIC: u32 = 3;
const PHANDLE_SYSCON: u32 = 4;

/// Interrupt numbers on the CPU-local interrupt controller (mcause codes).
const IRQ_M_SOFT: u32 = 3;
const IRQ_M_TIMER: u32 = 7;
const IRQ_S_EXT: u32 = 9;
const IRQ_M_EXT: u32 = 11;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceTreeConfig {
    pub ram_base: u64,
    pub ram_size: u64,
    pub bootargs: String,
    /// Physical start and end (exclusive) of an initrd, if loaded.
    pub initrd: Option<(u64, u64)>,
    pub timebase_hz: u32,
}

impl DeviceTreeConfig {
    pub fn new(ram_base: u64, ram_size: u64) -> Self {
        Self { ram_base, ram_size, bootargs: "console=ttyS0".to_string(), initrd: None, timebase_hz: TIMEBASE_HZ }
    }
}

fn node_name(kind: &str, address: u64) -> String {
    format!("{kind}@{address:x}")
}

/// Generates the board's device tree blob.
pub fn virt_device_tree(config: &DeviceTreeConfig) -> Vec<u8> {
    let uart = format!("/soc/{}", node_name("serial", UART_BASE));
    let mut w = FdtWriter::new();
    w.begin_node("");
    w.property_u32("#address-cells", 2);
    w.property_u32("#size-cells", 2);
    w.property_strings("compatible", &["hart,virt", "riscv-virtio"]);
    w.property_string("model", "hart RV64GC virt");

    w.begin_node("aliases");
    w.property_string("serial0", &uart);
    w.end_node();

    w.begin_node("chosen");
    w.property_string("bootargs", &config.bootargs);
    w.property_string("stdout-path", &uart);
    if let Some((start, end)) = config.initrd {
        w.property_u64("linux,initrd-start", start);
        w.property_u64("linux,initrd-end", end);
    }
    w.end_node();

    w.begin_node(&node_name("memory", config.ram_base));
    w.property_string("device_type", "memory");
    w.property_u64s("reg", &[config.ram_base, config.ram_size]);
    w.end_node();

    w.begin_node("cpus");
    w.property_u32("#address-cells", 1);
    w.property_u32("#size-cells", 0);
    w.property_u32("timebase-frequency", config.timebase_hz);
    w.begin_node("cpu@0");
    w.property_u32("phandle", PHANDLE_CPU0);
    w.property_string("device_type", "cpu");
    w.property_u32("reg", 0);
    w.property_string("status", "okay");
    w.property_string("compatible", "riscv");
    w.property_string("riscv,isa", ISA_STRING);
    w.property_string("riscv,isa-base", "rv64i");
    w.property_strings("riscv,isa-extensions", ISA_EXTENSIONS);
    w.property_string("mmu-type", "riscv,sv39");
    w.begin_node("interrupt-controller");
    w.property_u32("#interrupt-cells", 1);
    w.property_empty("interrupt-controller");
    w.property_string("compatible", "riscv,cpu-intc");
    w.property_u32("phandle", PHANDLE_CPU0_INTC);
    w.end_node();
    w.end_node();
    w.begin_node("cpu-map");
    w.begin_node("cluster0");
    w.begin_node("core0");
    w.property_u32("cpu", PHANDLE_CPU0);
    w.end_node();
    w.end_node();
    w.end_node();
    w.end_node();

    w.begin_node("soc");
    w.property_u32("#address-cells", 2);
    w.property_u32("#size-cells", 2);
    w.property_string("compatible", "simple-bus");
    w.property_empty("ranges");

    w.begin_node(&node_name("clint", CLINT_BASE));
    w.property_strings("compatible", &["sifive,clint0", "riscv,clint0"]);
    w.property_u64s("reg", &[CLINT_BASE, CLINT_SIZE]);
    w.property_u32s("interrupts-extended", &[PHANDLE_CPU0_INTC, IRQ_M_SOFT, PHANDLE_CPU0_INTC, IRQ_M_TIMER]);
    w.end_node();

    w.begin_node(&node_name("plic", PLIC_BASE));
    w.property_strings("compatible", &["sifive,plic-1.0.0", "riscv,plic0"]);
    w.property_u64s("reg", &[PLIC_BASE, PLIC_SIZE]);
    w.property_u32s("interrupts-extended", &[PHANDLE_CPU0_INTC, IRQ_M_EXT, PHANDLE_CPU0_INTC, IRQ_S_EXT]);
    w.property_empty("interrupt-controller");
    w.property_u32("#interrupt-cells", 1);
    w.property_u32("#address-cells", 0);
    w.property_u32("riscv,ndev", PLIC_NDEV);
    w.property_u32("phandle", PHANDLE_PLIC);
    w.end_node();

    w.begin_node(&node_name("serial", UART_BASE));
    w.property_string("compatible", "ns16550a");
    w.property_u64s("reg", &[UART_BASE, UART_SIZE]);
    w.property_u32("clock-frequency", UART_CLOCK_HZ);
    w.property_u32("interrupt-parent", PHANDLE_PLIC);
    w.property_u32("interrupts", UART_IRQ);
    w.end_node();

    w.begin_node(&node_name("test", SYSCON_BASE));
    w.property_strings("compatible", &["sifive,test1", "sifive,test0", "syscon"]);
    w.property_u64s("reg", &[SYSCON_BASE, SYSCON_SIZE]);
    w.property_u32("phandle", PHANDLE_SYSCON);
    w.end_node();

    w.end_node(); // soc

    w.begin_node("poweroff");
    w.property_string("compatible", "syscon-poweroff");
    w.property_u32("regmap", PHANDLE_SYSCON);
    w.property_u32("offset", 0);
    w.property_u32("value", FINISHER_PASS);
    w.end_node();

    w.begin_node("reboot");
    w.property_string("compatible", "syscon-reboot");
    w.property_u32("regmap", PHANDLE_SYSCON);
    w.property_u32("offset", 0);
    w.property_u32("value", FINISHER_RESET);
    w.end_node();

    w.end_node(); // root
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fdt::Fdt;

    fn tree() -> Fdt {
        let mut cfg = DeviceTreeConfig::new(0x8000_0000, 256 << 20);
        cfg.initrd = Some((0x8400_0000, 0x8410_0000));
        Fdt::parse(&virt_device_tree(&cfg)).unwrap()
    }

    #[test]
    fn cpu_describes_rv64gc_sv39() {
        let fdt = tree();
        let cpu = fdt.root.find("/cpus/cpu@0").unwrap();
        let isa = cpu.property_string("riscv,isa").unwrap();
        assert!(isa.starts_with("rv64imafdc"));
        assert_eq!(cpu.property_string("mmu-type"), Some("riscv,sv39"));
        assert_eq!(fdt.root.find("/cpus").unwrap().property_u32("timebase-frequency"), Some(10_000_000));
        let intc = cpu.child("interrupt-controller").unwrap();
        assert_eq!(intc.property_string("compatible"), Some("riscv,cpu-intc"));
        assert_eq!(intc.property_u32("phandle"), Some(PHANDLE_CPU0_INTC));
    }

    #[test]
    fn memory_and_chosen() {
        let fdt = tree();
        let mem = fdt.root.find("/memory@80000000").unwrap();
        assert_eq!(mem.property_cells("reg"), Some(vec![0, 0x8000_0000, 0, 0x1000_0000]));
        let chosen = fdt.root.find("/chosen").unwrap();
        assert_eq!(chosen.property_string("bootargs"), Some("console=ttyS0"));
        assert_eq!(chosen.property_string("stdout-path"), Some("/soc/serial@10000000"));
        assert_eq!(chosen.property_cells("linux,initrd-start"), Some(vec![0, 0x8400_0000]));
        assert!(fdt.root.find(chosen.property_string("stdout-path").unwrap()).is_some());
    }

    #[test]
    fn devices_at_qemu_virt_addresses() {
        let fdt = tree();
        let reg = |path: &str| fdt.root.find(path).unwrap().property_cells("reg").unwrap();
        assert_eq!(reg("/soc/clint@2000000"), vec![0, 0x0200_0000, 0, 0x1_0000]);
        assert_eq!(reg("/soc/plic@c000000"), vec![0, 0x0c00_0000, 0, 0x60_0000]);
        assert_eq!(reg("/soc/serial@10000000"), vec![0, 0x1000_0000, 0, 0x100]);
        assert_eq!(reg("/soc/test@100000"), vec![0, 0x10_0000, 0, 0x1000]);
        let clint = fdt.root.find("/soc/clint@2000000").unwrap();
        assert_eq!(clint.property_cells("interrupts-extended"), Some(vec![2, 3, 2, 7]));
        let plic = fdt.root.find("/soc/plic@c000000").unwrap();
        assert_eq!(plic.property_cells("interrupts-extended"), Some(vec![2, 11, 2, 9]));
        let uart = fdt.root.find("/soc/serial@10000000").unwrap();
        assert_eq!(uart.property_string("compatible"), Some("ns16550a"));
        assert_eq!(uart.property_u32("interrupt-parent"), Some(PHANDLE_PLIC));
        assert_eq!(uart.property_u32("interrupts"), Some(10));
        let poweroff = fdt.root.find("/poweroff").unwrap();
        assert_eq!(poweroff.property_u32("regmap"), Some(PHANDLE_SYSCON));
        assert_eq!(poweroff.property_u32("value"), Some(0x5555));
    }

    /// dtc must accept the blob without warnings when it is installed.
    #[test]
    fn dtc_accepts_blob_if_available() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let blob = virt_device_tree(&DeviceTreeConfig::new(0x8000_0000, 256 << 20));
        let child = Command::new("dtc")
            .args(["-I", "dtb", "-O", "dts", "-o", "/dev/null", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let Ok(mut child) = child else {
            eprintln!("dtc not installed; skipping");
            return;
        };
        child.stdin.take().unwrap().write_all(&blob).unwrap();
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "dtc failed: {stderr}");
        assert!(stderr.trim().is_empty(), "dtc warnings: {stderr}");
    }
}
