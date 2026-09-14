/* Higher-half kernel linker script (DECISIONS.md D4: base 0xffffffff80000000). */

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
    . = 0xffffffff80000000;

    /* Section-boundary symbols (brief M1-T4): `mm::vmm::init_kernel_space`
       reads these to map the kernel image W^X (text RX, rodata R+NX,
       data+bss RW+NX) instead of guessing section layout. Every symbol
       below sits on a `MAXPAGESIZE` (4 KiB) boundary, so each region maps
       cleanly with no other section sharing its first/last page. */
    __text_start = .;

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    __text_end = .;

    __rodata_start = .;

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    __rodata_end = .;

    __data_start = .;

    .data : {
        *(.data .data.*)

        /* Limine request structs: keep them, and keep the markers either
           side of them so the bootloader can find the whole block. */
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))

        /* The linker's own Global Offset Table (brief M1-T4 debugging:
           even with `relocation-model=static`, LLD still emits a small
           `.got` -- e.g. for calls into functions the linker resolves via
           a GOT-relative relocation, such as `core::fmt`'s per-type
           `LowerHex`/`Display::fmt` impls -- and, left unplaced here, it
           silently lands *after* `.bss`, outside `__data_end`, meaning
           `mm::vmm::init_kernel_space` never maps it: the first `{:x}`/
           `{}` of a runtime value after the CR3 switch takes a page fault
           reading it. Must stay inside `:data`, before `.bss` (see that
           section's own comment below). */
        *(.got)
        *(.got.plt)
    } :data

    /* .bss must be the last thing mapped into :data, or the file gains a
       lot of unnecessary zero bytes. */
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    __data_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
    }
}
