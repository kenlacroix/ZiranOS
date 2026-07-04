//! Tasks, context switching, and the cooperative scheduler — Milestone 9.
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
//!   - Commit 1: `switch_context` proven in isolation (a hand-driven round-trip).
//!   - **Commit 2 (here):** a round-robin `Scheduler` over the M8 heap and
//!     cooperative `yield_now`. `kernel_main` is registered as the idle task
//!     (id 0), only resumed when no other task is ready; worker tasks alternate
//!     by voluntarily yielding. The delicate parts are the *idle fallback*, and
//!     the rule that the scheduler lock is **dropped before** `switch_context`
//!     (holding it across the switch would deadlock the resumed task).
//!   - Commit 3 (M9b): the PIT and preemption, reusing this same switch.

use crate::interrupts;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

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

/// The idle task's index. `kernel_main` is registered here; it is never placed
/// in the ready queue and is resumed only when nothing else is runnable.
const IDLE: usize = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    Running,
    Finished,
}

/// A schedulable task: its own stack, and the parked stack pointer that *is* its
/// suspended execution. `saved_sp` is meaningful only while the task is not
/// running (a running task's stack pointer lives in the real `rsp`). `_stack`
/// exists purely to keep the backing memory alive; a task's stack must outlive
/// every switch that runs on it, so the `Task` owns it for its whole life.
struct Task {
    id: u64,
    state: State,
    saved_sp: u64,
    _stack: Box<[u8]>,
}

impl Task {
    /// Create a task that will begin executing at `entry` the first time it is
    /// switched to. We fabricate the exact stack image `switch_context` expects
    /// to restore, so that its six pops and final `ret` hand control to
    /// `task_trampoline` with `entry` waiting in `r15`.
    fn new(id: u64, entry: extern "C" fn()) -> Task {
        let stack = alloc::vec![0u8; STACK_SIZE].into_boxed_slice();
        let base = stack.as_ptr() as u64;

        // Round the top DOWN to a 16-byte boundary. The heap only guarantees
        // 8-byte alignment, but the System V ABI requires the stack to be
        // 16-byte aligned at a `call`. With the return-address slot placed at
        // `top - 8`, `switch_context`'s `ret` leaves rsp exactly at `top`
        // (16-aligned) on entry to `task_trampoline` — see the frame diagram in
        // `boot/switch.asm`. Getting this wrong is the classic silent triple
        // fault (verified under GDB in commit 1).
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

        Task { id, state: State::Ready, saved_sp, _stack: stack }
    }

    /// The idle task (id 0): `kernel_main` itself. It is already running on the
    /// boot stack, so it owns no fabricated stack and its `saved_sp` is filled in
    /// by the first switch *away* from it.
    fn idle() -> Task {
        Task {
            id: 0,
            state: State::Running,
            saved_sp: 0,
            _stack: alloc::vec![].into_boxed_slice(),
        }
    }
}

/// Round-robin scheduler over heap-allocated tasks. `ready` holds the indices of
/// runnable non-idle tasks in FIFO order; `current` is the running task. The idle
/// task is deliberately kept *out* of `ready` — it is the fallback, not a peer.
struct Scheduler {
    tasks: Vec<Task>,
    ready: VecDeque<usize>,
    current: usize,
}

impl Scheduler {
    fn with_idle() -> Scheduler {
        Scheduler {
            tasks: alloc::vec![Task::idle()],
            ready: VecDeque::new(),
            current: IDLE,
        }
    }
}

/// The one scheduler. `None` until [`init`] installs it. A `spin::Mutex` in the
/// house style, but with the strict discipline that its guard is **dropped before
/// every `switch_context`** — see [`yield_now`].
static SCHED: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Install the scheduler with `kernel_main` as the idle task. Call once, after
/// the heap is up.
fn init() {
    *SCHED.lock() = Some(Scheduler::with_idle());
}

/// Create a task and enqueue it as ready. Returns its id.
fn spawn(entry: extern "C" fn()) -> u64 {
    let mut guard = SCHED.lock();
    let sched = guard.as_mut().expect("task::spawn before init");
    let idx = sched.tasks.len();
    let id = idx as u64;
    sched.tasks.push(Task::new(id, entry));
    sched.ready.push_back(idx);
    id
}

/// Hand the CPU to the next runnable task. Picks the next ready task (falling
/// back to the idle task when none remain), requeues the outgoing task unless it
/// is idle or finished, then switches. Used both cooperatively (a task calling it
/// voluntarily) and by [`preempt`] from the timer IRQ.
///
/// Two disciplines keep this correct on a single core:
///
///   1. **Drop the lock before switching.** Everything the switch needs — a raw
///      pointer to the outgoing task's `saved_sp` and the incoming task's
///      `saved_sp` value — is extracted while the lock is held, then the guard is
///      dropped *before* `switch_context`. Holding it across the switch would
///      deadlock the instant the resumed task called back into the scheduler.
///   2. **Disable interrupts across the critical section.** Once tasks run with
///      interrupts on (Milestone 9b), a timer tick could otherwise fire while we
///      hold the scheduler lock, re-enter through `preempt`, and deadlock. We
///      clear IF for the lock *and* the switch, then restore it — to its prior
///      value — once resumed. (A preempted task's true IF is restored later by
///      `iretq`; a cooperative caller's by this `restore`.)
pub fn yield_now() {
    let flags = interrupts::save_and_disable();

    let switch: Option<(*mut u64, u64)> = {
        let mut guard = SCHED.lock();
        let sched = guard.as_mut().expect("task::yield_now before init");

        let old = sched.current;
        let next = sched.ready.pop_front().unwrap_or(IDLE);

        if next == old {
            // Only ourselves is runnable (idle yielding into an empty queue). Do
            // not "switch to self": `new_sp` would be a *stale* parked pointer.
            None
        } else {
            // Requeue the outgoing task unless it is the idle task or has finished.
            if old != IDLE && sched.tasks[old].state != State::Finished {
                sched.tasks[old].state = State::Ready;
                sched.ready.push_back(old);
            }
            sched.tasks[next].state = State::Running;
            sched.current = next;

            let old_sp_ptr = &mut sched.tasks[old].saved_sp as *mut u64;
            let new_sp = sched.tasks[next].saved_sp;
            Some((old_sp_ptr, new_sp))
        }
    }; // guard dropped here — before the switch

    if let Some((old_sp_ptr, new_sp)) = switch {
        // SAFETY: `old_sp_ptr` points at the outgoing task's `saved_sp` inside the
        // scheduler's `Vec`, which is stable (we never spawn mid-switch); `new_sp`
        // is the incoming task's parked stack, fabricated by `Task::new` or
        // written by a prior `switch_context`. The lock is released, so the
        // resumed task may re-enter the scheduler freely.
        unsafe { switch_context(old_sp_ptr, new_sp) };
    }

    interrupts::restore(flags);
}

/// Preemptive reschedule, driven by the timer IRQ (see `interrupt_dispatch`).
/// Mechanically identical to [`yield_now`] — the timer simply forces the switch a
/// cooperative task would otherwise make on its own. Safe to call from the IRQ
/// handler because `switch_context` is an ordinary `extern "C"` call: the
/// interrupted task's caller-saved registers are already preserved in the
/// `InterruptContext` on its stack, and when the task is later resumed, control
/// unwinds back through here to `isr_common`'s `iretq`, which restores its full
/// register state and interrupt flag.
pub fn preempt() {
    yield_now();
}

/// Retire the current task and yield forever. Called by `task_trampoline` when a
/// task's entry function returns. `#[no_mangle]`/`extern "C"` so the assembly can
/// `call task_exit`. Never returns: a finished task is never requeued, so
/// `yield_now` will not switch back to it.
#[no_mangle]
pub extern "C" fn task_exit() -> ! {
    // This task is retiring and will never be resumed. Disable interrupts for
    // good (we never restore) so we cannot be preempted while holding a lock — the
    // serial lock during the message below, or the scheduler lock. A task parked
    // forever while still holding a lock would wedge every other task. The task we
    // switch to restores its own interrupt flag (via `iretq` or its own yield).
    interrupts::save_and_disable();

    let finished_id = {
        let mut guard = SCHED.lock();
        let sched = guard.as_mut().expect("task::task_exit before init");
        let cur = sched.current;
        sched.tasks[cur].state = State::Finished;
        sched.tasks[cur].id
    };
    crate::serial_println!("[m9] task {} ran to completion and retired", finished_id);
    yield_now();
    // Unreachable: yield_now switched to another task and will never pick this
    // finished one again. Halt as a safety net rather than fall off the stack.
    crate::hlt_loop();
}

// --- Cooperative self-test --------------------------------------------------
//
// Two worker tasks that alternate by yielding. Their combined output is a shared
// string we assert is a strict "ABAB..." — the proof they interleave. Each worker
// also carries a checksum and a heap `Box` *across every yield* and re-checks
// them after resuming, which proves the switch preserved its callee-saved state
// and that heap allocation interleaved with switching does not corrupt anything.

/// How many turns each worker takes.
const ITERS: u64 = 4;

/// The interleaved output, built up by the workers; `None` until the test starts.
static SEQ: Mutex<Option<String>> = Mutex::new(None);
/// Per-worker final checksums, indexed by a small slot (workers 1 and 2 -> 0/1).
static CHECKSUMS: Mutex<[u64; 2]> = Mutex::new([0; 2]);

/// The checksum a worker accumulates across its `ITERS` turns. Pure function of
/// `ITERS`, so both workers must end with the same value — computed independently
/// in [`cooperative_self_test`] for the assertion.
fn fold_checksum() -> u64 {
    let mut acc: u64 = 1;
    let mut i = 0;
    while i < ITERS {
        acc = acc.wrapping_mul(31).wrapping_add(i);
        i += 1;
    }
    acc
}

fn worker_body(slot: usize, letter: char) {
    let mut acc: u64 = 1;
    for i in 0..ITERS {
        // Append our letter, releasing the lock before yielding (holding it
        // across a yield would deadlock the other worker).
        SEQ.lock().as_mut().expect("SEQ uninitialised").push(letter);
        acc = acc.wrapping_mul(31).wrapping_add(i);

        // `acc` and this heap Box must survive the switch intact.
        let carried = Box::new(acc);
        yield_now();
        assert_eq!(*carried, acc, "task: heap value corrupted across a context switch");
        drop(carried);
    }
    CHECKSUMS.lock()[slot] = acc;
}

extern "C" fn worker_a() {
    worker_body(0, 'A');
}
extern "C" fn worker_b() {
    worker_body(1, 'B');
}

/// Prove the cooperative scheduler works, over serial. Installs the scheduler
/// (with `kernel_main` as the idle task), spawns two workers, and enters the
/// round-robin by yielding. Control returns here once both workers finish and the
/// idle task is resumed. Runs while interrupts are still masked, before the PIC.
pub fn cooperative_self_test() {
    init();
    *SEQ.lock() = Some(String::new());

    spawn(worker_a);
    spawn(worker_b);

    // Enter the round-robin. Returns once both workers have finished.
    yield_now();

    let seq = SEQ.lock().take().expect("SEQ vanished");
    let expected: String = (0..ITERS * 2)
        .map(|i| if i % 2 == 0 { 'A' } else { 'B' })
        .collect();
    assert_eq!(
        seq, expected,
        "cooperative scheduler: tasks did not alternate cleanly (got {seq})"
    );

    let expected_acc = fold_checksum();
    let checksums = *CHECKSUMS.lock();
    assert_eq!(checksums[0], expected_acc, "worker A: state lost across switches");
    assert_eq!(checksums[1], expected_acc, "worker B: state lost across switches");

    crate::serial_println!(
        "[ok] scheduler: 2 tasks alternated cleanly ({}) over {} switches each; \
         callee-saved state and heap survived every switch",
        seq,
        ITERS
    );
    crate::serial_println!("M9: cooperative scheduler online");
}

// --- Preemptive self-test (Milestone 9b) ------------------------------------
//
// The proof that preemption is real and not secretly cooperative: two tasks that
// NEVER call yield_now. The "waiter" spins on a flag; the "setter" flips it. The
// waiter can make progress only if the timer interrupt forcibly takes the CPU
// away from it and runs the setter. If preemption were broken the waiter would
// spin forever and the boot test would time out — so merely reaching the marker
// proves the timer preempts.

static PREEMPT_FLAG: AtomicBool = AtomicBool::new(false);
static WAITER_SAW_FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn preempt_waiter() {
    // A task must have interrupts enabled to be preempted, and a freshly
    // bootstrapped task starts with them off (the switch into it ran with IF
    // clear). Enable them, then spin with NO yield — only the timer can break us
    // out of this loop.
    interrupts::enable();
    while !PREEMPT_FLAG.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    WAITER_SAW_FLAG.store(true, Ordering::Release);
}

extern "C" fn preempt_setter() {
    interrupts::enable();
    // A little bounded work so several timer ticks pass (interleaving us with the
    // waiter), then set the flag the waiter is spinning on. Also never yields.
    let mut acc: u64 = 0;
    for i in 0..3_000_000u64 {
        acc = acc.wrapping_add(i);
        core::hint::spin_loop();
    }
    core::hint::black_box(acc);
    PREEMPT_FLAG.store(true, Ordering::Release);
}

/// Prove preemption, over serial. Builds a fresh scheduler with two non-yielding
/// tasks and enters it. Because neither task ever yields, the only thing that can
/// interleave them is the timer IRQ — so completion *is* the proof. Runs after
/// the PIT is programmed and interrupts are enabled.
pub fn preemptive_self_test() {
    // Build the whole run with interrupts OFF, so no half-set-up state (only one
    // task spawned so far) can be preempted into — that could hang. Interrupts
    // come back on when kernel_main yields into the scheduler; each worker then
    // enables its own.
    let was = interrupts::save_and_disable();
    init();
    PREEMPT_FLAG.store(false, Ordering::Release);
    WAITER_SAW_FLAG.store(false, Ordering::Release);
    spawn(preempt_waiter);
    spawn(preempt_setter);
    interrupts::restore(was);

    // Enter the scheduler as the idle task. Returns once both workers finish.
    yield_now();

    assert!(
        WAITER_SAW_FLAG.load(Ordering::Acquire),
        "preemption failed: the non-yielding waiter never saw the setter run"
    );
    let ticks = crate::pit::ticks();
    assert!(ticks > 0, "timer never fired: no ticks recorded");

    crate::serial_println!(
        "[ok] scheduler: preemption works -- a task that never yields was \
         interrupted by the timer and another task ran ({} ticks elapsed)",
        ticks
    );
    crate::serial_println!("M9: preemptive scheduler online");
}
