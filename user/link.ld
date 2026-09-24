/* OtterOS userspace linker script (brief M2-T3, DECISIONS.md D18): static,
   non-PIE ELF64 linked at a fixed low address, W^X segments, page-aligned.
   Shared by every user/ crate (libotter's `build.rs`, one per bin crate,
   passes `-T` pointing here). */

OUTPUT_FORMAT(elf64-x86-64)

ENTRY(_start)

PHDRS
{
    text    PT_LOAD;
    rodata  PT_LOAD;
    data    PT_LOAD;
}

SECTIONS
{
    /* DECISIONS.md D18: user binaries link at 0x0000_0000_0040_0000. */
    . = 0x400000;

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)

        /* rust-lld emits a small .got/.got.plt even at
           `relocation-model=static` (see kernel/linker-x86_64.ld's
           identical note); keep it inside the writable data segment,
           before .bss. */
        *(.got)
        *(.got.plt)
    } :data

    /* .bss must be the last thing mapped into :data, or the file gains a
       lot of unnecessary zero bytes on disk. */
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment)
    }
}
