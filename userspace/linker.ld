ENTRY(_start)

SECTIONS {
    . = 0x400000;

    .text : {
        *(.text)
    }

    /DISCARD/ : {
        *(.eh_frame)
        *(.comment)
    }
}
