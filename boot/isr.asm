; boot/isr.asm -- the 256 interrupt service routine (ISR) entry stubs.
;
; This is the piece that makes Milestone 4 possible, and it exists because of an
; awkward truth: when the CPU takes an interrupt or exception, it does NOT follow
; the normal C calling convention. It pushes a small frame (see below) and jumps
; to a handler that must eventually return with `iretq`, not `ret`. Rust cannot
; express that entry/exit shape on stable, so we write a thin assembly shim per
; vector, normalise the stack into one predictable layout, then call ordinary
; Rust with a pointer to that layout.
;
; What the CPU pushes automatically, on the stack, before reaching our stub:
;
;     +--------+  <- higher addresses
;     |   SS   |
;     |  RSP   |   (the interrupted code's stack pointer)
;     | RFLAGS |
;     |   CS   |
;     |  RIP   |   (the instruction to resume at)
;     | [err]  |   <- ONLY for some exceptions (see error-code list below)
;     +--------+  <- lower addresses, where our stub starts
;
; The problem: some exceptions push an error code and some don't. To give the
; Rust handler ONE struct layout, stubs for the no-error-code vectors push a
; dummy 0, so from the common code's point of view every frame looks identical.
; Each stub also pushes its own vector number, so the handler knows what fired.

extern interrupt_dispatch      ; the Rust dispatcher: fn(*const InterruptContext)
global isr_stub_table          ; array of 256 stub addresses, read by Rust

section .text
bits 64

; A stub for a vector that the CPU does NOT give an error code: push a fake 0 so
; the frame matches the error-code case, then push the vector, then join common.
%macro isr_no_err 1
isr_stub_%+%1:
    push 0
    push %1
    jmp isr_common
%endmacro

; A stub for a vector that DOES arrive with a CPU-pushed error code: the error
; code is already on the stack, so only push the vector.
%macro isr_err 1
isr_stub_%+%1:
    push %1
    jmp isr_common
%endmacro

; Generate all 256 stubs. The vectors that carry a CPU error code are a fixed
; set defined by the architecture: 8 (#DF), 10 (#TS), 11 (#NP), 12 (#SS),
; 13 (#GP), 14 (#PF), 17 (#AC), 21 (#CP), 29 (#VC), 30 (#SX). Everything else
; gets a dummy so the layout stays uniform.
%assign v 0
%rep 256
    %if v == 8 || v == 10 || v == 11 || v == 12 || v == 13 || v == 14 || v == 17 || v == 21 || v == 29 || v == 30
        isr_err v
    %else
        isr_no_err v
    %endif
    %assign v v+1
%endrep

; The shared tail. On entry the stack (low -> high) is: vector, error_code, then
; the CPU frame. We save every general-purpose register so the handler sees the
; full interrupted state and so we can restore it exactly on the way out.
isr_common:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    ; The System V ABI requires the direction flag clear on entry to C code.
    ; The interrupted code may have left it set, so clear it before calling Rust.
    cld

    ; The System V ABI passes the first argument in RDI. RSP now points at the
    ; base of everything we just pushed -- exactly the InterruptContext struct
    ; the Rust side expects -- so hand it over as a pointer.
    mov rdi, rsp
    call interrupt_dispatch

    ; Restore in the exact reverse order.
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax

    ; Drop the vector and error code (8 bytes each) so RSP points back at the
    ; CPU-pushed RIP, then return from the interrupt. `iretq` restores RIP, CS,
    ; RFLAGS, RSP and SS in one atomic step -- the whole reason a plain `ret`
    ; would be wrong here.
    add rsp, 16
    iretq

; A flat table of the 256 stub addresses. Rust reads this to fill the IDT,
; instead of us hand-writing 256 entries in two languages.
section .rodata
isr_stub_table:
%assign v 0
%rep 256
    dq isr_stub_%+v
    %assign v v+1
%endrep

section .note.GNU-stack noalloc noexec nowrite progbits
