/* linker.ld -- how the boot objects and the Rust kernel become one image.
 *
 * The layout has two hard constraints:
 *   1. The Multiboot2 header must land within the first 32 KiB of the file, so
 *      GRUB can find it. We give it its own section, first, and KEEP it (the
 *      linker's garbage collector would otherwise drop it -- nothing *calls* a
 *      header).
 *   2. GRUB loads a Multiboot kernel at the conventional 1 MiB mark, so we set
 *      the load address there. Below 1 MiB is a minefield of BIOS/legacy memory.
 */

ENTRY(_start)

SECTIONS {
    . = 1M;

    /* Boot: Multiboot header first, then the 32/64-bit boot trampoline code. */
    .boot :
    {
        KEEP(*(.multiboot_header))
    }

    .text :
    {
        *(.text .text.*)
    }

    .rodata :
    {
        *(.rodata .rodata.*)
    }

    .data :
    {
        *(.data .data.*)
    }

    .bss :
    {
        *(.bss .bss.*)
        *(COMMON)
    }

    /* Rust/LLVM emit exception-handling and metadata sections we have no use for
     * in a `panic = "abort"` kernel. Drop them so they cannot bloat or misalign
     * the image. */
    /DISCARD/ :
    {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
    }
}
