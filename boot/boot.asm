; boot/boot.asm -- 32-bit entry, the bridge from GRUB to 64-bit long mode.
;
; This is Milestone 1 ("The first 512 bytes") and half of Milestone 2 in one
; file. GRUB drops us here in 32-bit protected mode. Before we can run the
; 64-bit Rust kernel we must, in order:
;
;   1. Confirm we were actually loaded by a Multiboot2 loader.
;   2. Confirm the CPU supports CPUID, then that it supports long mode at all.
;   3. Build a set of page tables (long mode requires paging to be on).
;   4. Turn on PAE, point CR3 at those tables, set the EFER long-mode bit,
;      then flip the paging bit in CR0 -- which activates long mode.
;   5. Load a 64-bit GDT and far-jump into a 64-bit code segment.
;
; Every failure path lands in `error`, which paints a two-character code to the
; top-left of the screen and halts, so a boot that dies here is still legible
; instead of silently triple-faulting.

global _start
global mb1_trampoline
extern long_mode_start

section .text
bits 32

; --- Browser (v86) entry: a Multiboot1 -> Multiboot2 trampoline --------------
; GRUB/QEMU enter at `_start` via the Multiboot2 header and leave a real MB2 info
; pointer in EBX. The web teaching tool's emulator (v86) only speaks Multiboot1,
; so its header (in multiboot_header.asm) sends it here instead. v86 hands off
; like an MB1 loader: EAX = 0x2BADB002, EBX -> an *MB1* info struct. Our kernel
; only parses MB2, so rather than teach it a second format we forge an MB2
; handoff: set EAX to the magic `check_multiboot` wants and point EBX at a static
; MB2 structure describing v86's memory, then fall straight into `_start`. GRUB
; never executes this (it jumps to `_start` directly).
mb1_trampoline:
    mov eax, 0x36d76289          ; the Multiboot2 loader magic check_multiboot wants
    mov ebx, mb2_synth           ; our hand-built MB2 info structure (in .rodata)
    jmp _start

_start:
    ; GRUB does not give us a stack; establish one immediately.
    mov esp, stack_top

    ; EBX holds the physical address of the Multiboot2 info structure. Stash it
    ; in EDI so it survives into long mode as the first C-ABI argument, ready for
    ; when the kernel wants the memory map (Milestone 6).
    mov edi, ebx

    call check_multiboot
    call check_cpuid
    call check_long_mode

    call set_up_page_tables
    call enable_paging

    ; Long mode is on, but we are still in a 32-bit code segment. Load the
    ; 64-bit GDT and far-jump to reload CS with a 64-bit descriptor.
    lgdt [gdt64.pointer]
    jmp gdt64.code_segment:long_mode_start

    ; Unreachable.
    hlt

; --- Sanity checks -----------------------------------------------------------

; Multiboot2 loaders leave this magic value in EAX. If it is missing we were
; started some other way and none of our assumptions hold.
check_multiboot:
    cmp eax, 0x36d76289
    jne .no_multiboot
    ret
.no_multiboot:
    mov al, "M"
    jmp error

; CPUID is detected by attempting to flip bit 21 (ID) of EFLAGS. If it sticks,
; CPUID is available.
check_cpuid:
    pushfd
    pop eax
    mov ecx, eax
    xor eax, 1 << 21
    push eax
    popfd
    pushfd
    pop eax
    push ecx            ; restore original EFLAGS
    popfd
    cmp eax, ecx
    je .no_cpuid
    ret
.no_cpuid:
    mov al, "C"
    jmp error

; Ask CPUID whether extended function 0x80000001 exists, then whether its LM
; (long mode) feature bit is set.
check_long_mode:
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_long_mode
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29     ; LM bit
    jz .no_long_mode
    ret
.no_long_mode:
    mov al, "L"
    jmp error

; --- Paging setup ------------------------------------------------------------

; Identity-map the first 1 GiB of physical memory using 2 MiB huge pages, so
; virtual address == physical address for everything the kernel touches early.
; One P4 entry -> one P3 entry -> one P2 table whose 512 entries each map 2 MiB.
set_up_page_tables:
    ; P4[0] -> P3
    mov eax, page_table_l3
    or eax, 0b11                  ; present | writable
    mov [page_table_l4], eax

    ; P3[0] -> P2
    mov eax, page_table_l2
    or eax, 0b11                  ; present | writable
    mov [page_table_l3], eax

    ; Fill all 512 P2 entries with 2 MiB huge-page mappings.
    mov ecx, 0
.map_p2:
    mov eax, 0x200000            ; 2 MiB
    mul ecx                      ; eax = 2MiB * ecx  (physical address of page)
    or eax, 0b10000011          ; present | writable | huge
    mov [page_table_l2 + ecx * 8], eax
    inc ecx
    cmp ecx, 512
    jne .map_p2
    ret

; Turn the tables on and enter long mode.
enable_paging:
    ; CR3 must hold the physical address of the P4 table.
    mov eax, page_table_l4
    mov cr3, eax

    ; Enable Physical Address Extension (CR4.PAE, bit 5) -- required for long mode.
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    ; Set the Long Mode Enable bit (bit 8) in the EFER MSR (0xC0000080).
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    ; Enable paging (CR0.PG, bit 31). The instant this bit is set with LME on and
    ; PAE on, the CPU activates long mode (compatibility submode for now).
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax
    ret

; --- Error reporting ---------------------------------------------------------

; Print "ERR: X" in white-on-red at the top-left of the VGA buffer, where X is
; the single-character code left in AL, then halt.
error:
    mov dword [0xb8000], 0x4f524f45   ; "ER"
    mov dword [0xb8004], 0x4f3a4f52   ; "R:"
    mov dword [0xb8008], 0x4f204f20   ; "  "
    mov byte  [0xb800a], al           ; the specific error character
    hlt

; --- Read-only data: the 64-bit GDT ------------------------------------------

section .rodata
gdt64:
    dq 0                                              ; mandatory null descriptor
.code_segment: equ $ - gdt64
    ; A single flat 64-bit ring-0 code segment. The important bits:
    ;   43 = executable, 44 = descriptor type (code/data),
    ;   47 = present,    53 = 64-bit (long) mode.
    dq (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)
.pointer:
    dw $ - gdt64 - 1                                  ; limit (size - 1)
    dq gdt64                                          ; base address

; --- Static Multiboot2 info structure, for the browser trampoline only --------
; Referenced by `mb1_trampoline` above. GRUB/QEMU never use this — they pass a
; real map. This one describes v86's machine, which the page configures with
; 64 MiB of RAM, so the Milestone 6 parser has a valid MB2 memory map to read in
; the emulator (it will report ~63 MiB usable, versus QEMU's ~121 MiB). Layout
; matches src/multiboot.rs: an 8-byte structure header, a type-6 memory-map tag
; with 24-byte entries, then the end tag.
align 8
mb2_synth:
    dd mb2_synth_end - mb2_synth        ; total_size
    dd 0                                 ; reserved
.mmap:
    dd 6                                 ; tag type: memory map
    dd .mmap_end - .mmap                 ; tag size
    dd 24                                ; entry_size (authoritative stride)
    dd 0                                 ; entry_version
    ; low RAM: 0 .. 639 KiB, usable
    dq 0x00000000
    dq 0x0009FC00
    dd 1                                 ; kind = usable
    dd 0
    ; EBDA + legacy hole: 639 KiB .. 1 MiB, reserved
    dq 0x0009FC00
    dq 0x00100000 - 0x0009FC00
    dd 2                                 ; kind = reserved
    dd 0
    ; main RAM: 1 MiB .. 64 MiB, usable
    dq 0x00100000
    dq 0x04000000 - 0x00100000
    dd 1                                 ; kind = usable
    dd 0
.mmap_end:
    ; end tag (type 0, size 8); already 8-byte aligned here
    dd 0
    dd 8
mb2_synth_end:

; --- Statically reserved page tables and the boot stack ----------------------

section .bss
align 4096
page_table_l4:
    resb 4096
page_table_l3:
    resb 4096
page_table_l2:
    resb 4096
stack_bottom:
    resb 4096 * 4        ; 16 KiB boot stack
stack_top:

; Mark the stack non-executable (silences the linker's exec-stack warning). We
; never execute from the stack; this is the correct, explicit declaration.
section .note.GNU-stack noalloc noexec nowrite progbits
