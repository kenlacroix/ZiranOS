; boot/usermode.asm -- Milestone 13: drop to ring 3, and come back.
;
; This is the two-way door between ring 0 and ring 3. Going down is a hand-built
; `iretq`: the CPU only ever *lowers* privilege by returning from an interrupt,
; so to *enter* ring 3 for the first time we fabricate the exact frame an
; interrupt would have pushed (SS, RSP, RFLAGS, CS, RIP with ring-3 selectors)
; and `iretq` into it. Coming back is not symmetric: the ring-3 blob asks to
; exit via `int 0x80` (SYS_EXIT), and the Rust syscall handler rewrites that
; interrupt's saved frame to point back *here*, at `usermode_resume`, on the
; kernel stack we stashed on the way in. So the return rides the ISR's own
; `iretq` (see boot/isr.asm) -- no second mechanism, no longjmp.
;
; Written in assembly, not a Rust `#[naked]` fn, for the same reason switch.asm
; is: the stack shape has to be exactly ours, with no compiler prologue between
; the saved registers and the `ret`.

global usermode_enter
global usermode_resume
global kernel_rsp

section .bss
; The kernel RSP captured just before we drop to ring 3 — the point SYS_EXIT
; restores so `usermode_resume` unwinds the saved registers and returns into the
; Rust caller. One qword; read by the syscall handler.
kernel_rsp: resq 1

section .text
bits 64

; extern "C" fn usermode_enter(entry: u64 /*rdi*/, user_stack_top: u64 /*rsi*/)
;
; Save the callee-saved registers the System V ABI says we must preserve, stash
; the resulting RSP, then `iretq` into ring 3 at `entry` with `user_stack_top`.
; Does not "return" in the ordinary sense: control comes back only when SYS_EXIT
; redirects its interrupt return to `usermode_resume` below.
usermode_enter:
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov [kernel_rsp], rsp          ; remember where to resume (points at r15)

    ; Fabricate the ring-3 interrupt-return frame, pushed low-to-high so that
    ; `iretq` pops RIP, CS, RFLAGS, RSP, SS in that order off the top.
    push 0x23                      ; SS  = user data selector (0x20) | RPL 3
    push rsi                       ; RSP = top of the user stack
    push 0x2                       ; RFLAGS = reserved bit 1 only; IF=0 so the
                                   ;   ring-3 excursion is non-preemptible (there
                                   ;   is no ring-3 save/restore in the scheduler
                                   ;   yet -- a deliberate M13 deferral)
    push 0x1b                      ; CS  = user code selector (0x18) | RPL 3
    push rdi                       ; RIP = the blob's entry point
    iretq                          ; -> ring 3, CPL 3

; SYS_EXIT rewrites the saved interrupt frame so the ISR's `iretq` lands here, in
; ring 0, with RSP already restored to [kernel_rsp]. Unwind the saved registers
; and return to whoever called usermode_enter.
usermode_resume:
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret

; Non-executable stack marker (matches the other boot objects).
section .note.GNU-stack noalloc noexec nowrite progbits
