OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

PHDRS {
    text   PT_LOAD FLAGS(5);   /* R-X */
    rodata PT_LOAD FLAGS(4);   /* R-- */
    data   PT_LOAD FLAGS(6);   /* RW- */
}

SECTIONS {
    /* Higher-half kernel */
    . = 0xffffffff80000000;

    .text : { *(.text .text.*) } :text

    . = ALIGN(4K);
    .rodata : {
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4K);
    .data : { *(.data .data.*) } :data

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) }
}

