# Milestone 9 — timer + scheduling — eng-plan

Locks the technical approach before building. Grounded in a four-way research
fan-out (the IRQ0/PIC integration points, the PIT programming brief, the
context-switch/scheduler design, and the docs/"done" conventions). Companion
concept doc: `docs/concepts/timer-and-scheduling.md` (written during the build).

The milestone teaches exactly one thing: **a task is nothing but a saved stack
pointer, and switching tasks is swapping `rsp` (+ the callee-saved registers) so
`ret` resumes someone else's call stack.** Preemption adds one clause: an
interrupt can force that swap against a task's will.

## 0. Scope-guard (do this first)

Scheduling is where a teaching kernel quietly turns into a real one. The traps,
and the deferrals (verdict **Hold**, recorded in the office-hours pass):

- **PIT, not the local APIC.** IRQ0 rides the exact 8259 path the keyboard (IRQ1)
  already uses — remap done, `send_eoi` done, `isr_stub_32` already generated.
  The APIC timer needs LAPIC enable + MMIO at `0xFEE00000` + calibration against
  another clock + moving off the 8259s: that is its own future milestone (the
  "real hardware" gravity well PLAN §6 names), with **zero** marginal learning
  over the PIT for a *first* timer. Named future milestone, not a "while I'm
  here."
- **Ring 0, one kernel stack per task, no TSS/IST.** We switch on the current
  kernel stack. The TSS/IST work (so a stack overflow reports instead of
  triple-faulting) stays deferred exactly as STATUS records; `ist = 0` today.
- **Round-robin, no priorities, no sleep/block/join/IPC.** Those are a real
  scheduler. Two tasks, a FIFO ready-queue that *happens* to generalize to N.
- **Cooperative (9a) is demoable on its own, before preemptive (9b).** If 9b
  stalls in the triple-fault swamp, 9a is still a shippable, teachable artifact.
- **One context-switch implementation.** Preemption reuses the cooperative
  `switch_context` verbatim (see §3, Approach B) — no second, iretq-frame-based
  switch. That single decision is what keeps 9b small.

Verdict: **Hold**, deliberately minimal. APIC timer recorded as a named future
milestone so it can't leak back into M9 mid-build.

## 1. Approach

Two new asm files, two new Rust modules, and small edits to three existing files.
Built as two demoable sub-steps.

**9a — cooperative:**

- `boot/switch.asm` — `switch_context(old_sp: *mut u64 /*rdi*/, new_sp: u64
  /*rsi*/)`, `extern "C"`, house style (external asm like `isr.asm`, **not**
  `#[naked]` — it removes all prologue/ABI ambiguity). Pushes the 6 callee-saved
  regs (`rbp, rbx, r12, r13, r14, r15`), stores `rsp` into `*old_sp`, loads `rsp`
  from `new_sp`, pops the 6, `ret`. Plus `task_trampoline` (see §3) as the
  first-run return target.
- `src/task.rs` (or `src/sched.rs`) — the `Task` struct and a `spin::Mutex`-wrapped
  round-robin `Scheduler`:
  ```rust
  enum State { Ready, Running, Finished }
  struct Task { id: u64, state: State, saved_sp: u64, _stack: Box<[u8]> }
  struct Scheduler { tasks: Vec<Task>, ready: VecDeque<usize>, current: usize }
  ```
  `spawn(entry: fn() -> !)` allocates a `Box<[u8]>` stack, **fabricates the
  initial frame** (§3) so the first switch-in `ret`s into `task_trampoline`, and
  enqueues the task. `yield_now()` funnels through `pick_next` → `switch_context`.
  The boot thread (`kernel_main`) is registered as **task 0** (idle task: an
  empty `_stack`, it already owns the boot stack; its body `hlt`s when the queue
  drains).
- `lib.rs` — `mod task;`, register task 0, `spawn` the demo tasks, then enter the
  scheduler.

**9b — preemptive (adds, does not replace):**

- `src/pit.rs` — mirrors `pic.rs` style. `init(target_hz)` programs channel 0,
  mode 2 (rate generator), lo/hi, binary, and a divisor; plus a
  `static TICKS: AtomicU64` and `handle_interrupt()` doing `fetch_add`.
  ```
  PIT_FREQ_HZ   = 1_193_182
  PIT_COMMAND   = 0x43   PIT_CH0_DATA = 0x40
  command byte  = 0x34   // ch0, lo/hi, mode 2, binary
  divisor(100Hz)= 11932 = 0x2E9C   (low 0x9C first, then high 0x2E)
  actual        = 1_193_182 / 11932 = 100.00 Hz
  ```
- `src/pic.rs:80` — unmask IRQ0: master mask `0xFD → 0xFC`
  (`0xff & !((1<<1) | (1<<0))`); update the comment that currently says the timer
  is intentionally masked.
- `src/interrupts.rs` — `const TIMER_VECTOR: usize = pic::PIC1_OFFSET as usize;`
  next to `KEYBOARD_VECTOR`, and a `match` arm **above** the spurious-range guard
  (`0x20..0x2F`) that does `pit::handle_interrupt()` → `pic::send_eoi(0)` →
  `sched::preempt()`.
- `boot/switch.asm` `task_trampoline` — `sti` first (a cooperatively-bootstrapped
  task starts with IF cleared; without this it can never be preempted again).

## 2. Preconditions

**`switch_context` (asm), on entry:** long mode; called as an ordinary
`extern "C"` function, so the compiler has already spilled caller-saved regs
around the call and the return address is on the stack. It assumes `new_sp`
points at a stack whose top 7 slots are exactly `[r15, r14, r13, r12, rbx, rbp,
return-addr]` (§3). **On exit:** the outgoing task's `rsp` is saved to `*old_sp`;
execution continues on the incoming task's stack at its saved return address,
with its callee-saved regs restored. Callable both cooperatively and from inside
`interrupt_dispatch` (Approach B).

**Fabricated stack (task creation):** the `Box<[u8]>` gives an **8-byte-aligned**
region (the heap rounds to `align_of::<ListNode>() = 8`, *not* 16). The fabricated
frame **must** put the return-address slot on a 16-byte boundary — compute the
top as `(base + len) & !0xF`, then push the 7 slots below it. A misaligned
`rsp` is the #1 triple-fault (§4).

**`pit::init` / unmask / `sti` ordering:** program the PIT divisor *before*
unmasking IRQ0 (else the first tick arrives with the counter in its power-on
18.2 Hz default — harmless but sloppy), and unmask before the single global `sti`
in `kernel_main`. Interrupt gates are `0x8E` (IF cleared on entry), so the timer
handler runs with interrupts disabled — the brief scheduler critical section is
therefore uncontended on this single core.

**`preempt` from the ISR:** on entry `isr_common` has already saved all 15 GPRs
into an `InterruptContext` on the *outgoing* task's stack. `send_eoi(0)` **must**
happen before the switch (else the outgoing task's tick is never acknowledged and
the PIC goes silent). **On exit** (when this task is later resumed) control
returns up through `interrupt_dispatch` → `isr_common` restore → `iretq`, which
restores the saved RFLAGS (IF set) — so a task preempted by the timer resumes
correctly. A *brand-new* task resumed via the cooperative `ret` path instead gets
IF from `task_trampoline`'s `sti`.

## 3. Data flow

**Cooperative switch (9a):**
```
task A body ── yield_now() ──┐
   lock scheduler            │
   old=current; mark A Ready; ready.push_back(old)
   next=ready.pop_front(); current=next; mark B Running
   old_sp_ptr = &mut tasks[old].saved_sp ; new_sp = tasks[next].saved_sp
   UNLOCK scheduler          │  ← drop the guard BEFORE switching (§4 deadlock)
   switch_context(old_sp_ptr, new_sp)
       push rbp/rbx/r12-r15 ; *old_sp_ptr = rsp ; rsp = new_sp ; pop ; ret
                             └── resumes task B where it last yielded
```

**Parked-stack layout** (`new_sp` / `saved_sp` points at the `r15` slot; `ret`
pops the return address, which must be 16-aligned):
```
higher addr  [ return address ]  ← 16-byte aligned  (task_trampoline, first run)
             [ saved rbp ]
             [ saved rbx ]
             [ saved r12 ]
             [ saved r13 ]
             [ saved r14 ]
saved_sp →   [ saved r15 ]        (fabricated = entry fn ptr, read by trampoline)
lower addr
```

**Task bootstrap** — fabricate the frame above with `return address =
task_trampoline`, `r15 = entry_fn`, the other five = 0:
```asm
task_trampoline:
    sti                 ; new task runs with interrupts on (needed once preemptive)
    mov  rdi, r15       ; r15 fabricated to hold entry fn ptr
    call rdi            ; run the task body (fn() -> !)
.park: cli ; hlt ; jmp .park   ; never `ret` — nothing valid is below the frame
```

**Preemptive switch (9b):** IRQ0 → `isr_stub_32` → `isr_common` (saves 15 GPRs)
→ `interrupt_dispatch(ctx)`:
```
TIMER_VECTOR => { pit::handle_interrupt(); pic::send_eoi(0); sched::preempt(); }
        preempt()  → same pick_next + switch_context as yield_now
```
Because `switch_context` is an ordinary `extern "C"` call, the caller-saved regs
are already in the `InterruptContext` and the callee-saved regs of
`interrupt_dispatch`'s frame are preserved by the switch. When this task is
resumed, execution returns through `interrupt_dispatch` → `isr_common` restore →
`iretq`. **One switch, both paths — no fabricated iretq frame.**

## 4. Edge cases

- **16-byte stack misalignment (the #1 triple-fault).** System V requires
  `rsp ≡ 8 (mod 16)` on function entry (return slot 16-aligned). The heap gives
  only 8-byte alignment. Round the frame top down: `top = (base + len) & !0xF`.
  Do **not** trust the `Box` pointer. First `movaps` on a misaligned stack →
  `#GP` → triple fault.
- **Clobbered / mismatched callee-saved set.** `switch_context`'s push list and
  pop list must be exact mirror images of `{rbp, rbx, r12, r13, r14, r15}`, and
  the fabricated frame must match that order. An off-by-one makes the first `ret`
  jump to `0` or into a register value — a fault far from the cause.
- **A task body that returns.** Nothing valid is below the entry frame. Entry
  signature is `fn() -> !`, and `task_trampoline` catches a stray return with
  `cli;hlt` — never a `ret`.
- **Missing EOI (9b).** Forget `send_eoi(0)` and the PIC's in-service bit for
  IRQ0 stays set: exactly **one** tick fires, then silence. The classic
  first-timer bug — looks like a hang, not a fault.
- **Missing `sti` in the trampoline (9b).** A freshly-bootstrapped task resumed
  cooperatively starts with IF cleared and is never preempted again. Also a
  silent hang, not a fault.
- **Holding the scheduler lock across the switch.** Task A locks, switches into
  B, B tries to lock → spin forever on one core. Extract the two `sp` values
  under the lock, **drop the guard, then `switch_context`.** This is the concrete
  answer to the "revisit re-entrancy at M9" notes the frame/paging/heap modules
  all defer to.
- **Empty ready-queue.** All tasks `Finished` → switch to task 0, the idle task,
  which `hlt`s. Never leave `current` pointing at a `Finished` task.
- **Stack overflow has no guard page.** The heap has no guard pages, so a task
  that overflows its `Box<[u8]>` stack silently corrupts an adjacent heap
  allocation instead of faulting. Acceptable for two toy tasks; documented as a
  known limitation (same spirit as the heap's "no coalescing" note), and the
  concrete motivation for the deferred TSS/IST + guard-page work.
- **QEMU PIT timing is approximate.** QEMU does not emulate the 1.193 MHz crystal
  cycle-accurately; ticks jitter and can coalesce, especially across `hlt`.
  Expect the *rate* right to a few percent over a second; do not treat inter-tick
  spacing as precise. Fine for counting ticks; the precision path is the later
  APIC/TSC milestone.

## 5. Verification plan (working vs. accidentally working)

A scheduler's failure mode is looking fine while subtly corrupting state. The
self-tests are built to separate real from accidentally-working, over serial:

1. **9a — clean cooperative alternation.** Two tasks each `print!` their letter
   and `yield_now()`; serial shows `ABABAB…`. Proof the switch preserves and
   resumes each call stack.
2. **9a — survives a deep call stack.** Each task calls a helper that recurses a
   few frames *before* yielding. A botched callee-saved save/restore shows up as
   garbage output here, not clean letters — this is the guard against
   "accidentally working" with shallow stacks.
3. **9a — reclaim/ownership honesty.** A parked task must resume correctly *after*
   other heap allocations happen (the scheduler owns every `Box<[u8]>` for the
   kernel's life; a freed/reused stack would corrupt). Allocate after the first
   yield, then confirm continued clean alternation.
4. **9b — preemption is real, not secretly cooperative.** Two tasks in tight
   loops with **no `yield_now()` at all** still interleave. If they don't, the
   "switch" was cooperative. This is the direct analogue of M8's reclaim test —
   the one thing a cooperative-only scheduler provably cannot do.
5. **9b — the tick heartbeat.** `static TICKS`; print `[timer] Ns` every 100th
   tick → ~one line/second in wall-clock time for 10+ s without the stream
   stopping (a stop = missing EOI) and without the keyboard going dead (a botched
   mask byte). Cross-check with QEMU `-d int`: repeated `v=20` entries at ~100 Hz,
   independent of the handler.
6. **CI marker** `M9: preemptive scheduler online` printed from the scheduler
   init; `Makefile` `MILESTONE_MARKER` updated from the M8 marker to this one.
   (9a interim marker `M9: cooperative scheduler online` while 9b is unbuilt.)

**GDB plan for the triple-fault class (office-hours Q5):** these faults are
silent and instant, so — not recompile-and-pray — `make debug` + break on
`switch_context`, single-step the first switch, and `info registers` before and
after the `ret`: verify `rsp` is 16-aligned at the return slot and the six
callee-saved regs round-trip. The `/investigate` 3-attempt rule applies.

## 6. Build order (each its own small commit, QEMU-tested)

De-risks by proving the switch mechanism in isolation before the timer can hide a
switch bug behind a timing bug.

1. **`switch_context` + a hand-driven single switch.** `boot/switch.asm`,
   `Task`, fabricated frame + `task_trampoline`, but driven by a *direct*
   `switch_context` call from `kernel_main` into one task that prints once and
   `hlt`s. Verify in GDB that the first switch lands in the task with a 16-aligned
   stack. This isolates the alignment/frame-fabrication risk (the historical
   time-sink here) from scheduler logic.
2. **Cooperative round-robin (9a).** `Scheduler` (`Vec` + `VecDeque`), task 0 as
   idle, `spawn`, `yield_now` with extract-then-unlock. Self-tests 1–3.
   Interim marker `M9: cooperative scheduler online`. Demoable, shippable.
3. **PIT + preemption (9b).** `src/pit.rs`, unmask IRQ0, the `TIMER_VECTOR`
   dispatch arm, trampoline `sti`. Self-tests 4–5. Final marker
   `M9: preemptive scheduler online`; `Makefile` marker updated.
4. **Docs + web.** `docs/concepts/timer-and-scheduling.md`; the `web/index.html`
   `SCREEN` line + real `n:"09"` `STEPS` step + `ROAD` update; then
   `/kernel-review` → `/retro` → `/document-milestone`.

## 7. Open unknowns (honest)

- **Exact fabricated-frame slot for `r15`/entry fn.** The trampoline reads the
  entry fn from `r15`; will confirm in GDB that the fabricated `saved_sp` points
  at the `r15` slot and the pops land the entry ptr in `r15` before `ret`. If the
  register-passing feels fragile, fall back to stashing the entry ptr at a known
  stack offset the trampoline reads directly.
- **`fn() -> !` vs `extern "C" fn`** for task entries — leaning plain `fn() -> !`
  (the trampoline does the `call`); will confirm the ABI lands cleanly.
- **Whether to keep the step-1 hand-driven-switch commit** or squash it into the
  cooperative commit — decide at commit time (leaning keep, as honest de-risking,
  like M7's translate step and M8's throwaway-bump step).
- **PIT mode 2 vs 3 under QEMU** — planning mode 2 (rate generator, one clean IRQ
  per period); will glance at `-d int` cadence to confirm QEMU delivers it as
  expected. Mode 3 is a one-line footnote (the BIOS's old 18.2 Hz tick).
- **Idle-task `hlt` vs spin** when the queue drains — `hlt` is correct (wait for
  the next IRQ0), but confirm a preempt from inside `hlt` resumes cleanly.
