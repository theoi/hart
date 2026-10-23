OUTPUT_ARCH("riscv")
ENTRY(_start)
SECTIONS
{
  . = 0x80000000;
  .text.init : { *(.text.init) }
  .text : { *(.text) *(.text.*) }
  . = ALIGN(0x1000);
  .tohost : { *(.tohost) }
  . = ALIGN(0x1000);
  .data : { *(.data) *(.data.*) *(.sdata) *(.sdata.*) }
  .bss : { *(.bss) *(.bss.*) *(.sbss) *(.sbss.*) }
  _end = .;
}
