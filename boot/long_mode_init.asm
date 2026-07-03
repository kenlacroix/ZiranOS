; boot/long_mode_init.asm -- the first code that runs in true 64-bit long mode.
;
; The far jump at the end of boot.asm reloaded CS with a 64-bit descriptor, so
; we are finally executing 64-bit instructions. The data segment registers,
; though, still hold whatever GRUB left in them. In long mode they are mostly
; ignored, but stale values can trip up later transitions, so we zero them.
;
; Then we call into Rust. This is the seam described in Milestone 2,
; "Handing off to Rust": the last line of assembly before the language changes.

global long_mode_start
extern kernel_main

section .text
bits 64
long_mode_start:
    ; Reload all data segment registers with the null selector (0). Long mode
    ; treats DS/ES/SS/FS/GS as flat, so 0 is the correct, deliberate value.
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    ; Hand off. EDI (now RDI) still carries the Multiboot info pointer stashed in
    ; boot.asm, so it arrives as kernel_main's first C-ABI argument if/when the
    ; Rust signature grows one.
    call kernel_main

    ; kernel_main is declared `-> !` and must never return. If it somehow does,
    ; do not fall off the end of the world -- halt forever.
.halt:
    hlt
    jmp .halt

; Mark the stack non-executable (silences the linker's exec-stack warning). We
; never execute from the stack; this is the correct, explicit declaration.
section .note.GNU-stack noalloc noexec nowrite progbits
