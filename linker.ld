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

    /* Mark the base of the loaded kernel image. The frame allocator (Milestone 6)
     * reads this as an *address* to carve the kernel's own frames out of usable
     * RAM, so it must never hand back memory the running kernel occupies. It sits
     * at exactly 1 MiB -- the load address -- because nothing of ours precedes
     * `.boot`. */
    kernel_start = .;

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

    /* The GOT/PLT. Left implicit, the linker places these *orphan* sections after
     * .data — i.e. after `data_end` below — so they fall outside the flat binary
     * the Multiboot1/`-kernel` loader copies ([kernel_start, data_end)). Code
     * dereferences GOT entries (e.g. inside core::fmt's argument thunks), so if the
     * GOT isn't loaded those pointers read as garbage and the first such call jumps
     * into the weeds (a #UD at a tiny address, then triple fault). GRUB's ELF loader
     * covers them via the LOAD segment regardless; naming them here pulls them
     * inside `data_end` so the flat path works too. */
    .got  : { *(.got) *(.got.*) }
    .got.plt : { *(.got.plt) }

    /* End of the initialized, file-backed image (everything before .bss, which is
     * zero-filled and not stored). The Multiboot1 header the browser emulator
     * uses points its `load_end_addr` here: the flat binary produced by `objcopy`
     * spans [kernel_start, data_end), and the loader zero-fills up to kernel_end. */
    data_end = .;

    .bss :
    {
        *(.bss .bss.*)
        *(COMMON)
    }

    /* Mark the end of the loaded kernel image, rounded up to a 4 KiB frame
     * boundary so the first frame the allocator hands out starts clean. `.bss`
     * is the last allocated section, and it already contains the boot page
     * tables and the 16 KiB boot stack (reserved in boot.asm), so everything the
     * running kernel needs falls inside [kernel_start, kernel_end). */
    . = ALIGN(4K);
    kernel_end = .;

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
