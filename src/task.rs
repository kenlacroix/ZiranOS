//! Tasks and context switching — Milestone 9.
//!
//! The single idea this milestone teaches: **a task is a saved stack pointer.**
//! A `Task` owns a chunk of memory to use as a stack, plus the one number that
//! matters when it is *not* running — `saved_sp`, the stack pointer where its
//! execution is parked. Switching tasks (`boot/switch.asm`'s `switch_context`)
//! swaps that pointer and the callee-saved registers; nothing else is needed to
//! make one CPU appear to run more than one thread of control.
//!
//! This file is built in stages (see `docs/planning/milestone-09-eng-plan.md`):
//!
//!   - **Commit 1 (here):** `switch_context` proven in isolation — a single task
//!     on a fresh heap stack that the kernel switches into and that switches
//!     straight back. No scheduler, no timer yet. The delicate part is
//!     `Task::new`, which fabricates a parked stack frame by hand so the first
//!     switch-in lands in `task_trampoline` with the entry fn in `r15`.
//!   - Commit 2 adds the round-robin `Scheduler` and cooperative `yield_now`.
//!   - Commit 3 (M9b) adds the PIT and preemption, reusing this same switch.

use alloc::boxed::Box;
use core::ptr::{addr_of, addr_of_mut};

extern "C" {
    /// Save the current callee-saved registers and stack pointer into `*old_sp`,
    /// then adopt `new_sp` and resume the task parked there. Defined in
    /// `boot/switch.asm`.
    fn switch_context(old_sp: *mut u64, new_sp: u64);
    /// First-run shim a fabricated task frame returns into. Defined in
    /// `boot/switch.asm`; we only ever take its address.
    fn task_trampoline();
}

/// Per-task kernel stack size: 16 KiB, matching the boot stack. Heap-allocated
/// (Milestone 8), so a handful of tasks fit comfortably in the 1 MiB heap.
const STACK_SIZE: usize = 16 * 1024;

/// A schedulable task: its own stack, and the parked stack pointer that *is* its
/// suspended execution. `saved_sp` is meaningful only while the task is not
/// running (a running task's stack pointer lives in the real `rsp`). `_stack`
/// exists purely to keep the backing memory alive; a task's stack must outlive
/// every switch that runs on it, so the `Task` owns it for its whole life.
pub struct Task {
    pub id: u64,
    pub saved_sp: u64,
    _stack: Box<[u8]>,
}

impl Task {
    /// Create a task that will begin executing at `entry` the first time it is
    /// switched to. We fabricate the exact stack image `switch_context` expects
    /// to restore, so that its six pops and final `ret` hand control to
    /// `task_trampoline` with `entry` waiting in `r15`.
    pub fn new(id: u64, entry: extern "C" fn() -> !) -> Task {
        let stack = alloc::vec![0u8; STACK_SIZE].into_boxed_slice();
        let base = stack.as_ptr() as u64;

        // Round the top DOWN to a 16-byte boundary. The heap only guarantees
        // 8-byte alignment, but the System V ABI requires the stack to be
        // 16-byte aligned at a `call`. With the return-address slot placed at
        // `top - 8`, `switch_context`'s `ret` leaves rsp exactly at `top`
        // (16-aligned) on entry to `task_trampoline` — see the frame diagram in
        // `boot/switch.asm`. Getting this wrong is the classic silent triple
        // fault, which is why commit 1 verifies it under GDB.
        let top = (base + STACK_SIZE as u64) & !0xF;

        // The seven fabricated slots, low address -> high:
        //   [0] r15 = entry   [1] r14   [2] r13   [3] r12   [4] rbx   [5] rbp
        //   [6] return address = task_trampoline
        // `saved_sp` points at slot 0 (r15); slot 6 sits at `top - 8`.
        let saved_sp = top - 7 * 8;

        // SAFETY: `[saved_sp, top)` is 56 bytes wholly inside the freshly
        // allocated, still-owned stack (`STACK_SIZE` is far larger), and 8-byte
        // aligned. We initialise every slot before the task can run.
        unsafe {
            let f = saved_sp as *mut u64;
            f.add(0).write(entry as usize as u64); // r15 -> entry fn
            f.add(1).write(0); // r14
            f.add(2).write(0); // r13
            f.add(3).write(0); // r12
            f.add(4).write(0); // rbx
            f.add(5).write(0); // rbp
            f.add(6).write(task_trampoline as *const () as u64); // return address
        }

        Task { id, saved_sp, _stack: stack }
    }
}

// --- Commit-1 scaffolding: a hand-driven round-trip -------------------------
//
// This proves `switch_context` works in both directions before any scheduler
// exists. It is deliberately minimal and gets replaced by the real `Scheduler`
// in commit 2. `RESUME_SP` is where the demo task switches *back* to — the
// kernel_main context parked by the outbound switch.

/// kernel_main's parked stack pointer, set by the outbound switch in
/// [`demo_roundtrip`] and read by [`demo_task`] to return control.
static mut RESUME_SP: u64 = 0;

/// The demo task: runs on its own stack, announces itself, and hands control
/// straight back to kernel_main. `extern "C"` and `-> !` so it matches the
/// trampoline's `call r15` and never falls off the bottom of a fabricated stack.
extern "C" fn demo_task() -> ! {
    crate::serial_println!("[m9] hello from task 1 -- running on its own 16 KiB heap stack");
    crate::println!("[m9] a second stack ran, on the CPU, and is about to hand control back");

    // Switch back to kernel_main. We never resume this task, so its own saved-sp
    // slot is a throwaway on this (soon-abandoned) stack.
    let mut discard: u64 = 0;
    // SAFETY: `RESUME_SP` was written by demo_roundtrip's outbound switch before
    // this task ever ran; single core, interrupts masked.
    unsafe {
        let resume = addr_of!(RESUME_SP).read();
        switch_context(&mut discard, resume);
    }
    // Unreachable: control returns into kernel_main at the outbound call site,
    // never here. Satisfy the `!` return type.
    crate::hlt_loop();
}

/// Commit-1 demonstration, called once from kernel_main: create a task, switch
/// into it, and let it switch straight back — a full round-trip through
/// `switch_context`. Runs while interrupts are still masked.
pub fn demo_roundtrip() {
    let task = Task::new(1, demo_task);

    // SAFETY: single-threaded boot with interrupts masked. The outbound switch
    // parks kernel_main's context in `RESUME_SP` and jumps into `task`; the task
    // switches back to that exact context. `task` (which owns the stack the task
    // runs on) stays alive until the end of this scope — i.e. past the whole
    // round-trip — because a stack freed mid-switch would be instant corruption.
    unsafe {
        switch_context(addr_of_mut!(RESUME_SP), task.saved_sp);
    }

    crate::serial_println!(
        "[m9] context switch round-tripped back into kernel_main (task {} parked)",
        task.id
    );
    drop(task);
}
