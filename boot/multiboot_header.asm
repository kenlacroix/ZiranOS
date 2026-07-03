; boot/multiboot_header.asm -- the Multiboot2 header.
;
; A Multiboot2-compliant bootloader (GRUB, here) scans the first 32 KiB of the
; kernel image for this magic structure. If it finds it, it loads us into
; 32-bit protected mode with a known machine state and jumps to our entry point,
; instead of us having to write a raw BIOS boot sector. That is the deliberate
; trade documented in PLAN.md section 4: understand the boot handoff, but do not
; get stuck hand-rolling a stage-1/stage-2 loader before the interesting work.
;
; The whole header is just a magic number, the architecture, a length, a
; checksum that makes the four dwords sum to zero, and a terminating tag.

section .multiboot_header
header_start:
    dd 0xe85250d6                ; Multiboot2 magic number
    dd 0                         ; architecture 0 = i386 (32-bit protected mode)
    dd header_end - header_start ; header length
    ; checksum: chosen so (magic + arch + length + checksum) == 0 mod 2^32
    dd 0x100000000 - (0xe85250d6 + 0 + (header_end - header_start))

    ; --- end tag: type = 0, flags = 0, size = 8 ---
    dw 0    ; type
    dw 0    ; flags
    dd 8    ; size
header_end:

; Mark the stack non-executable (silences the linker's exec-stack warning). We
; never execute from the stack; this is the correct, explicit declaration.
section .note.GNU-stack noalloc noexec nowrite progbits
