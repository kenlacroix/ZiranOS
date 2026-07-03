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

extern mb1_trampoline           ; 32-bit entry for the v86 path (boot.asm)
extern data_end                 ; end of file-backed image (linker.ld)
extern kernel_end               ; end of .bss, zero-fill target (linker.ld)

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

; --- Multiboot1 header, for the browser emulator (v86) -----------------------
; GRUB/QEMU boot via the Multiboot2 header above and ignore this one. The web
; teaching tool's v86 emulator only understands Multiboot *1*, and only its
; 32-bit "a.out kludge" load path -- its ELF loader is 32-bit-only and our image
; is ELF64. So we hand v86 explicit physical load addresses for a *flat* binary
; (built with objcopy; see the Makefile `web` target) and aim its entry at the
; 32-bit trampoline in boot.asm. GRUB's `multiboot2` command ignores this header;
; v86 ignores the MB2 one. Both fit in the first 32 KiB, as each loader requires.
align 8
mb1_header:
    dd 0x1BADB002                                   ; Multiboot1 magic
    dd 1 << 16                                       ; flags: a.out kludge (load addrs valid)
    dd 0x100000000 - (0x1BADB002 + (1 << 16))       ; checksum -> (magic+flags+cksum) == 0
    dd mb1_header                                    ; header_addr: where this header loads
    dd 0x100000                                      ; load_addr: image base (1 MiB)
    dd data_end                                      ; load_end_addr: end of file data
    dd kernel_end                                    ; bss_end_addr: zero-fill up to here
    dd mb1_trampoline                                ; entry_addr: 32-bit entry

; Mark the stack non-executable (silences the linker's exec-stack warning). We
; never execute from the stack; this is the correct, explicit declaration.
section .note.GNU-stack noalloc noexec nowrite progbits
