; boot/switch.asm -- the cooperative context switch for Milestone 9.
;
; A "task" in this kernel is nothing more exotic than a saved stack pointer.
; Switching from one task to another is therefore just: save the registers the
; System V ABI says a function must preserve across a call, remember where this
; task's stack is, load the other task's stack, restore its registers, and
; return. The `ret` at the end lands us wherever the incoming task last stopped
; -- which is the whole trick.
;
; We write this in assembly (rather than a Rust `#[naked]` fn) for the same
; reason isr.asm exists: the exact prologue/epilogue and stack shape have to be
; ours, with no compiler-inserted spills to reason about. It links via `extern
; "C"` exactly like the ISR stubs.

extern task_exit               ; Rust: retire the current task, never returns

global switch_context
global task_trampoline

section .text
bits 64

; extern "C" fn switch_context(old_sp: *mut u64 /*rdi*/, new_sp: u64 /*rsi*/)
;
; Park the outgoing task and resume the incoming one. Because this is reached by
; an ordinary `call`, the compiler has already spilled the *caller*-saved
; registers around the call site, and the return address is already on the
; stack. So we save only the six *callee*-saved registers plus the stack pointer
; itself -- that is the entire machine state a cooperative switch must preserve.
switch_context:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov  [rdi], rsp     ; *old_sp = current rsp -- park the outgoing task here
    mov  rsp, rsi       ; adopt the incoming task's stack
    pop  r15            ; restore in the exact mirror of the pushes above
    pop  r14
    pop  r13
    pop  r12
    pop  rbx
    pop  rbp
    ret                 ; resume at the incoming task's saved return address

; task_trampoline -- the first-run shim every freshly spawned task starts in.
;
; A brand-new task has no real saved context to restore; `Task::new` fabricates
; one by hand so that the first `switch_context` into the task `ret`s to *here*
; with r15 holding the task's entry function. The fabricated stack looks like
; this (see src/task.rs), highest address at the top:
;
;     [ return address = task_trampoline ]  <- 16-byte aligned; `ret` lands here
;     [ rbp = 0 ]
;     [ rbx = 0 ]
;     [ r12 = 0 ]
;     [ r13 = 0 ]
;     [ r14 = 0 ]
;     [ r15 = entry_fn ]  <- saved_sp points HERE
;
; After the six pops, `ret` pops the return-address slot and leaves rsp exactly
; 16-byte aligned -- the alignment the System V ABI demands at a `call`, and the
; single thing most likely to triple-fault if the fabricated frame is off. The
; entry fn is `fn() -> !`; if it ever returned there is nothing valid below it,
; so we trap on a halt loop rather than execute a bogus `ret`.
;
; When the entry fn *returns* (worker tasks are bounded, so they do), we must not
; `ret` -- there is nothing valid below the fabricated frame. Instead we fall into
; `task_exit`, which marks this task Finished and yields to the next runnable one,
; never coming back. The halt loop is an unreachable safety net.
;
; NOTE (Milestone 9b): preemption will add an `sti` as the first instruction
; here, so a task bootstrapped via `ret` (which, unlike `iretq`, does not restore
; the interrupt flag) runs with interrupts enabled and can be preempted. It is
; deliberately omitted now: the cooperative self-test runs while interrupts are
; still masked, before the PIC is configured, so enabling them here would be
; premature.
task_trampoline:
    call r15           ; r15 was fabricated to hold the entry fn pointer
    call task_exit     ; the body returned: retire this task; does not return
.hang:
    cli
    hlt
    jmp .hang

section .note.GNU-stack noalloc noexec nowrite progbits
