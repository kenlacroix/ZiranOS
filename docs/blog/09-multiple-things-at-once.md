# Multiple things at once, sort of

*Milestone 9 — the timer and the first scheduler. The kernel learns to run more
than one thread of control, first by cooperation and then by force.*

> New to this? Read
> [docs/concepts/timer-and-scheduling.md](../concepts/timer-and-scheduling.md)
> alongside — it explains what a task actually is (a saved stack pointer), how a
> context switch swaps one call stack for another, and how a timer interrupt
> turns cooperation into preemption, from scratch.

Last milestone gave the kernel a heap: data structures that grow at runtime. This
one spends it. A scheduler needs somewhere to keep its list of runnable tasks and
somewhere to put each task's stack, and both live on the M8 heap. With memory in
hand, the kernel stops being a single straight-line program and starts being
*several* — two tasks taking turns on one CPU. First they take turns politely (a
task yields when it's ready); then a ~100 Hz timer interrupt takes the turn away
from a task that never intended to give it up. That second step — preemption — is
the one that makes it a real scheduler and not a coroutine trick.

### Milestone 9 — timer + scheduling — checklist

Think
- [x] office-hours — below
- [x] scope-guard — **Hold**, below

Plan
- [x] eng-plan — [docs/planning/milestone-09-eng-plan.md](../planning/milestone-09-eng-plan.md)

Build
- [x] Working, demoable state (cooperative *and* preemptive); `make` clean; header check passes
- [x] No new warnings; new `unsafe`/asm justified (the `switch_context` register save/restore and the fabricated first frame)

Debug (as needed)
- [x] The real debugging story captured — below (the honest version: there wasn't one)

Review
- [x] kernel-review — 3-lens pass (asm switch, scheduler locking, PIT/IRQ)

Security
- [x] No new attack surface (still ring 0, one shared kernel stack per switch, no privilege boundary) — /red-team N/A

Reflect
- [x] document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand.** How a CPU is made to run more than one
   thread of control. A "task" turns out to be almost nothing: a saved stack
   pointer. Switching tasks is just swapping `rsp` and the callee-saved registers
   so that the next `ret` resumes *someone else's* call stack instead of yours.
   Preemption adds exactly one idea on top: an interrupt can force that swap
   against a task's will, mid-computation, whether it wanted to yield or not.

2. **"Done" as observable behavior.** Two stages, two markers. **9a (cooperative):**
   two tasks that call `yield` produce a clean alternating `ABABAB` stream on
   serial, and the marker `M9: cooperative scheduler online`. **9b (preemptive):**
   two tasks in *tight loops with no yield* still interleave, there's a ~100 Hz
   tick heartbeat behind them, and the CI marker `M9: preemptive scheduler
   online`.

3. **The smallest version that still teaches it.** Two tasks, round-robin, fixed
   heap-allocated stacks. No priorities, no sleep, no join. And crucially the
   cooperative half (9a) is *itself* a complete, demoable teaching artifact — you
   can understand context switching fully before a single timer interrupt exists.

4. **Working vs. accidentally working.** Several traps here, each with a guard:
   - Alternation has to survive a **deep call stack**. A botched callee-saved
     save/restore doesn't crash politely — it prints garbage instead of clean
     letters. Clean `ABAB` through nested calls is the proof.
   - The preemptive demo must interleave tasks that contain **no `yield` at all**.
     If either task yields, the "preemption" was secretly cooperative and proves
     nothing.
   - A parked task must resume correctly *after other heap allocations have
     happened* — proof its saved state is really independent.
   - The classic accidental-success trap: it works for two ticks and then hangs.
     That's not "done," that's a **missing EOI** or a task that never re-enabled
     interrupts — one tick gets through and the line goes quiet.

5. **What most likely stalls this for days.** A triple-fault reboot with no stack
   trace. The usual causes: **16-byte stack misalignment** (the heap only
   guarantees 8-byte alignment, but the ABI wants 16 at a `call` boundary), a
   **mismatched push/pop set** in `switch_context`, or a **wrong fabricated frame**
   for a task that has never run. The mitigation is GDB, not recompile-and-pray:
   break on `switch_context`, single-step the very first switch, and watch `rsp`
   and the callee-saved registers with your own eyes.

6. **Drifting toward a non-goal?** Three temptations to watch. The **APIC timer**
   (the PIT is the scope-correct *first* timer — the APIC is a later milestone).
   **Blocking / sleep / IPC** (that's where a teaching scheduler turns into a real
   kernel). And **TSS/IST per-task stacks** (deferred — ring 0 runs on a single
   stack here). Ran `/scope-guard` → **Hold**.

## Scope-guard

Verdict: **Hold**. In scope and correctly sized. The three tempting expansions —
an APIC timer, TSS/IST per-task kernel stacks, and blocking/sleep/IPC — are all
deferred *outward* into named future work, not quietly smuggled in. Two tasks
behind a `VecDeque` that already generalizes to N gives the full insight without
building a workload manager. And staging cooperative-first is itself scope
discipline: 9a is a shippable, complete artifact even if preemption had stalled.
The APIC timer was written down as its own future milestone specifically so it
can't leak back in through the side door.

## Eng-plan (summary)

A hand-written context switch plus a small scheduler on the heap:

- **`boot/switch.asm`** holds `switch_context(old_rsp_ptr, new_rsp)` — push the
  callee-saved registers, save `rsp` into the outgoing task, load the incoming
  task's `rsp`, pop, `ret` — and `task_trampoline`, the landing pad a
  brand-new task returns into on its first ever switch.
- **`src/task.rs`** defines a `Task` (an id, a heap-allocated stack, a saved
  `rsp`) and, for a never-run task, **fabricates** the exact stack frame the
  switch expects to pop, so that its first `ret` lands in `task_trampoline` with
  the task's entry point in a known register.
- The **`Scheduler`** is a round-robin `VecDeque` of tasks living on the M8 heap,
  with an idle task 0 always present. `yield_now` picks the next task and calls
  the switch — dropping the scheduler lock *before* switching, never across it.
- **`src/pit.rs`** programs the 8254 PIT channel 0 in mode 2 at ~100 Hz, and the
  timer IRQ's handler calls `preempt()`, which reuses the same `switch_context`.

The staging isolated the risk the way M8 isolated its linker ceremony: get the
context switch provably correct in cooperative mode first, then add the timer.
Full plan in
[docs/planning/milestone-09-eng-plan.md](../planning/milestone-09-eng-plan.md).

## What got built

Six commits on `claude/ziran-os-plan-qgobid`:

- **M9 plan** — office-hours + scope-guard + eng-plan.
- **Step 1 — the switch.** `boot/switch.asm` (`switch_context` +
  `task_trampoline`) and `src/task.rs` (`Task` + the fabricated first frame).
  GDB-verified the highest-risk invariant *before any scheduler existed*: the new
  task's `rsp` was 16-byte aligned (`rsp=0x40008ff0`) and its trampoline received
  the entry point in the right register (`r15=&demo_task` at `task_trampoline`).
- **Step 2 — cooperative round-robin.** The `Scheduler` on the heap, idle task 0,
  and `yield_now` with the drop-lock-before-switch discipline. Marker advanced to
  `M9: cooperative scheduler online`.
- **Step 3 — preemption.** `src/pit.rs` (8254 mode 2, ~100 Hz, **divisor 11931**),
  IRQ0 unmasked in `src/pic.rs` (mask `0xFD` → `0xFC`), the `TIMER_VECTOR`
  dispatch arm in `src/interrupts.rs`, `preempt()` reusing `switch_context`,
  interrupt-safe scheduler locking, and tasks that self-enable interrupts.
- **M9 review.** A 3-lens `/kernel-review` (asm switch, scheduler locking,
  PIT/IRQ). The concurrency surface was confirmed correct; the pass added a PIT
  divisor guard, made spawn/init self-safe, and documented the deliberate limits
  (no guard page under task stacks, soft-float only).
- **Makefile** — CI boot marker advanced `M8` → `M9: cooperative scheduler
  online` → `M9: preemptive scheduler online`.

## Verification status (honest)

Boot-tested live under headless QEMU. The serial log:

```
M8: kernel heap online
[m9] task 1 ran to completion and retired
[m9] task 2 ran to completion and retired
[ok] scheduler: 2 tasks alternated cleanly (ABABABAB) over 4 switches each; callee-saved state and heap survived every switch
M9: cooperative scheduler online
Ziran OS: PIC remapped, timer + keyboard IRQs unmasked.
[ok] timer: PIT channel 0 at ~100 Hz (divisor 11931)
[m9] task 2 ran to completion and retired
[m9] task 1 ran to completion and retired
[ok] scheduler: preemption works -- a task that never yields was interrupted by the timer and another task ran (95 ticks elapsed)
M9: preemptive scheduler online
[boot test] PASS -- reached long mode and 'M9: preemptive scheduler online'
```

What each asserted line actually proves:

- **`ABABABAB` over 4 switches each** — cooperative interleaving works, *and* the
  callee-saved registers and the heap survived every switch. If the register
  save/restore were wrong this line would be garbage, not clean letters.
- **`PIT channel 0 at ~100 Hz (divisor 11931)`** — the timer was actually
  programmed (input clock 1193182 Hz ÷ 100 Hz ≈ divisor 11931), so a periodic
  interrupt now exists to preempt on.
- **The non-yielding waiter retiring, `95 ticks elapsed`** — the deterministic
  proof of preemption. A task that never calls `yield` finished anyway, which can
  *only* happen if the timer interrupt forced a switch away from a task holding the
  CPU. If preemption were broken, that waiter would spin forever and the boot test
  would **time out** rather than print this line.

The full 20-second run had no panics and no faults — the only exception raised was
the expected M4 `int3` breakpoint self-test.

## Retro

**Plan vs. reality.** The plan's *shape* held completely — cooperative-first, PIT
over APIC, an external-assembly switch, staged commits — with no mid-flight
revision. Where it was naive was in framing the preemption hazard as merely "EOI
ordering plus `sti` in the trampoline." The real hazard was structural: *any*
acquisition of the scheduler lock with interrupts enabled can deadlock under
preemption (the timer fires, re-enters the scheduler, and tries to take a lock the
interrupted task already holds). That forced an interrupt-safe `yield_now` the
plan hadn't called for. The plan had also assumed a `sti` inside the trampoline;
that would have leaked interrupts into the fully-masked cooperative phase, so the
design moved to **tasks enabling their own interrupts** — cleaner, and better as a
teaching artifact.

**What broke, and for how long.** Nothing. Zero debugging cycles, zero
triple-faults, no GDB bug-hunts. Each step booted correctly on essentially the
first try — including the context switch, which is exactly the thing office hours
warned would eat days.

**The root cause / the fix.** There was no bug, so there was no fix in the usual
sense. The closest thing was a decision made *on paper before writing code*:
recognizing that the naive lock discipline would deadlock the moment cooperative
yield (interrupts enabled, `IF=1`) coexisted with preemption, and pre-emptively
making the scheduler lock interrupt-safe. A design change that prevents a bug
leaves no debugging story behind — which is the point.

**The assumption worth re-examining (the lede).** *"Preemption is just cooperative
yield triggered by a timer."* Mechanically this is true — `preempt()` literally
calls `yield_now()`, there is exactly one switch implementation underneath both.
But that framing hides the whole subtlety: the timer changes *when* the scheduler
can be entered. It turns a benign, single-threaded lock into a **reentrant-deadlock
surface**, because now the scheduler can be entered from inside an interrupt, on
top of a task that was already inside the scheduler. The mechanical simplicity and
the concurrency danger are the same milestone wearing two faces.

**What earned its keep.** Two things. First, **GDB-first de-risking in commit 1** —
proving stack alignment (`rsp=0x40008ff0`) and the trampoline hand-off
(`r15=&demo_task`) *before any scheduler existed at all*, retiring the single
highest-risk item in isolation where nothing else could be blamed. Second, the
**deterministic preemption proof** — the waiter/setter flag pair that turns "did
preemption actually work?" into a binary CI gate that either prints its line or
hangs the boot test. No judgement call, no eyeballing timing.

**One thing to do differently.** Push the interrupt-flag / concurrency analysis
into the eng-plan's *edge-cases* section as a first-class item — something like
"every lock reachable from an IRQ handler must be `IF`-guarded" — instead of
letting it surface only in the verification prose. The design got it right, but it
got it right by luck of thinking about it while writing, not because the plan
demanded it.

## Takeaway for the next person

A task is a saved stack pointer, and a context switch is just swapping `rsp` and
the callee-saved registers so the next `ret` resumes someone else's work.
Preemption adds one idea on top — a timer interrupt that performs that same swap
against a task's will — and that one idea is deceptively deep: it doesn't change
*how* you switch, it changes *when* the scheduler can be entered, which is where
all the concurrency hazard lives. The honest test that you built preemption and
not a coroutine is a single line: a task that never yields finishes anyway.

Next: **Milestone 10 — the shell.** The kernel can now run more than one thing at
once, so the interactive shell won't be a straight-line loop bolted to the
keyboard driver — it'll run *as a task on this scheduler*, sharing the CPU with
whatever else is scheduled. The first user-facing program lands on top of the
machinery this milestone just built.
