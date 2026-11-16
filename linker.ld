OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

SECTIONS {
    /* Higher-half kernel. Each section starts on a page boundary so it
       can get its own permissions. */
    . = 0xffffffff80000000;
    __kernel_start = .;

    . = ALIGN(4K);
    .text : {
        __text_start = .;
        *(.text .text.*)
        . = ALIGN(4K);
        __text_end = .;
    }

    . = ALIGN(4K);
    .rodata : {
        __rodata_start = .;
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
        *(.rodata .rodata.*)
        . = ALIGN(4K);
        __rodata_end = .;
    }

    . = ALIGN(4K);
    .data : {
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(4K);
    }

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4K);
        __bss_end = .;
    }

    __kernel_end = .;

    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) }
}
