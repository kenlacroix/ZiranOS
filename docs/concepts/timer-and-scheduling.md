# Timer and scheduling, from first principles

*Companion to Milestone 9. Read this before `src/task.rs` and `boot/switch.asm` —
it explains what those files are *for* and why "just run two things at once"
hides the fact that on one CPU there is only ever one instruction pointer, one
stack pointer, one set of registers, and making it *look* like more takes real
sleight of hand. `src/task.rs` holds the `Task`, the `Scheduler`, and the two
kinds of yield; `boot/switch.asm` is the twenty-odd instructions that actually
swap one thread of control for another; `src/pit.rs` is the timer chip that
forces the swap. This builds directly on three siblings: [heap.md](heap.md) —
every task's stack is a `Box<[u8]>` from the M8 allocator, so this is the heap's
first real customer; and [interrupts.md](interrupts.md) with
[keyboard-and-pic.md](keyboard-and-pic.md) — the timer is **IRQ0**, arriving
through the exact same IDT → PIC → EOI machinery the keyboard already taught us.
The one thing you need coming in: after the earlier milestones we are in long
mode, the heap is up (so `Box` works), and the IDT and PIC are configured (so an
interrupt has somewhere to land).*

---

## The problem: one CPU, more than one thread of control

Everything the kernel has done so far runs in a single straight line. `kernel_main`
calls a function, which calls another, which returns; the stack grows and shrinks;
one instruction follows the last. Even interrupts, for all their drama, are still
that single line — the CPU takes a detour to a handler and comes right back to
where it was. There has only ever been *one* thread of control, and it has only
ever been in one place at a time.

But an operating system's whole job is to host *many* things that each believe they
have the machine to themselves — a shell reading a line, a timer counting, a task
crunching numbers. On a machine with one core there is exactly one CPU, one set of
registers, one `rsp`. You cannot actually run two things at once. What you can do is
run one for a while, freeze it *so completely* that it can be thawed later with no
way to tell it was ever paused, run the other, and swap back and forth fast enough
that both make progress. That illusion — many threads of control on one CPU — is
what a **scheduler** provides, and the mechanism underneath it is the **context
switch**.

Two questions have to be answered to build it, and this milestone answers them in
order:

- **What does it mean to "freeze" a running computation** so it can be resumed
  perfectly later? (The context switch — 9a, cooperative.)
- **What forces the swap** if a task is selfish and never volunteers to pause?
  (The timer — 9b, preemptive.)

## A task is a saved stack pointer

Here is the entire core insight, and it is smaller than you expect: **a suspended
task is nothing but a saved stack pointer.**

Think about what a running computation actually *is* at any instant. It is the
values in the registers, and the stack — the chain of call frames holding every
local variable and every return address, all the way down. Now notice that the
stack pointer, `rsp`, already *points at* the top of that chain. If, just before
pausing a task, you shove the few registers that aren't already on the stack *onto*
the stack, then the single number `rsp` locates the task's entire frozen state. Save
that one number and you have saved everything. That is why `Task` in `src/task.rs`
is so thin:

```rust
struct Task {
    id: u64,
    state: State,       // Ready, Running, or Finished
    saved_sp: u64,      // the parked stack pointer — the task's frozen self
    _stack: Box<[u8]>,  // the backing memory; kept alive, never touched by name
}
```

`saved_sp` is the whole trick. The doc-comment says it plainly: it "is meaningful
only while the task is not running (a running task's stack pointer lives in the real
`rsp`)." When a task runs, its state lives in the actual CPU. When it is parked, its
state lives on its stack, and `saved_sp` is the bookmark. `_stack` exists *purely* to
keep the `Box` alive — a task's stack must outlive every switch that runs on it, so
the `Task` owns it for its whole life. The underscore says "we never read this field
by name"; we reach its bytes only through `saved_sp`. Everything else — `id`, `state` —
is bookkeeping for the scheduler, not for the switch. **The switch touches exactly one
field.**

## The cooperative context switch

`switch_context` in `boot/switch.asm` is the operation that parks one task and resumes
another. Its signature is:

```rust
fn switch_context(old_sp: *mut u64, new_sp: u64);
```

"Save where *we* are into `*old_sp`, then go to where `new_sp` says the other task is."
The body is twenty instructions, and every one earns its place:

```asm
switch_context:
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov  [rdi], rsp     ; *old_sp = current rsp — park the outgoing task here
    mov  rsp, rsi       ; adopt the incoming task's stack
    pop  r15            ; restore in the exact mirror of the pushes above
    pop  r14
    pop  r13
    pop  r12
    pop  rbx
    pop  rbp
    ret                 ; resume at the incoming task's saved return address
```

Why only these six registers — `rbp, rbx, r12, r13, r14, r15`? Because
`switch_context` is reached by an ordinary `call`, and the System V ABI divides the
registers into two camps. The **caller-saved** registers (`rax`, `rcx`, `rdx`, `rsi`,
`rdi`, `r8`–`r11`) the compiler *already* spilled around the call site if it cared
about them — that is what "caller-saved" means. The **callee-saved** six are the ones
a called function must preserve, so those are exactly the ones `switch_context` must
carry across the swap. Save those plus `rsp` and you have saved the entire integer
machine state that matters — on this soft-float target. (SSE/x87 are off; more on that
in the limitations.)

Read the middle three lines as the actual moment of the switch. `mov [rdi], rsp` writes
the outgoing task's stack pointer — now sitting just below its six freshly-pushed
registers — into `*old_sp`. That task is now fully parked: its `saved_sp` bookmarks a
stack that has everything needed to resume it. `mov rsp, rsi` adopts the incoming
task's stack. From this instruction on, we are *on the other task's stack*. The six
pops then restore *its* callee-saved registers.

**The `ret` is the punchline.** A `ret` pops a return address off the stack and jumps
to it. But we are on the incoming task's stack now — so `ret` jumps to wherever *that*
task last stopped. If the incoming task previously paused inside its own call to
`yield_now`, `ret` lands back in *its* `yield_now`, which returns to *its* body, and
the task resumes as if the pause never happened. One `ret` instruction, and control has
crossed from one thread of control to another. That is the whole magic, and it is
nothing more than "return, but to someone else's call."

### The fabricated stack frame

The `ret` trick assumes the incoming task's stack already looks like a task that was
paused mid-`switch_context`. That is true for any task that has run before — it got
parked by a real `switch_context`. But a **brand-new** task has never run; its stack is
just 16 KiB of freshly-zeroed heap. So `Task::new` *fabricates by hand* the exact stack
image that `switch_context`'s six pops and final `ret` expect to find, so that switching
into a fresh task lands cleanly in its entry function.

The fabricated frame looks like this (highest address at the top), straight from the
diagram in `boot/switch.asm`:

```text
   higher addr
     top     ->  [ return address = task_trampoline ]  <- 16-byte aligned; `ret` lands here
   top - 8       [ rbp = 0 ]
                 [ rbx = 0 ]
                 [ r12 = 0 ]
                 [ r13 = 0 ]
                 [ r14 = 0 ]
   saved_sp  ->  [ r15 = entry fn ]   <- the six pops end here; saved_sp points HERE
   lower addr
```

`Task::new` writes exactly those seven `u64` slots:

```rust
let top = (base + STACK_SIZE as u64) & !0xF;  // round the top DOWN to 16 bytes
let saved_sp = top - 7 * 8;                    // seven slots below the top
// ...
f.add(0).write(entry as usize as u64);            // r15 -> entry fn
f.add(1).write(0);  // r14      f.add(4).write(0); // rbx
f.add(2).write(0);  // r13      f.add(5).write(0); // rbp
f.add(3).write(0);  // r12
f.add(6).write(task_trampoline as *const () as u64); // return address
```

Trace one switch into this frame. `switch_context` sets `rsp = saved_sp`, then pops
six times: `pop r15` reads slot 0 (loading `entry` into `r15`), `pop r14` … `pop rbp`
read the five zeros. After six pops `rsp` sits at slot 6, the return-address slot. `ret`
pops *that* — `task_trampoline` — and jumps there, with `r15` holding the entry function
pointer. The fresh task is now running inside `task_trampoline` with everything it needs.

### The 16-byte alignment that triple-faults if you get it wrong

Why round the top *down* to a 16-byte boundary (`& !0xF`), and why put the return slot at
`top - 8`? Because the System V ABI requires `rsp` to be 16-byte aligned *at the point of
a `call`* — equivalently, `rsp ≡ 8 (mod 16)` on entry to a function, since the `call`
pushed an 8-byte return address. The heap only guarantees **8-byte** alignment (it rounds
allocations to `align_of::<ListNode>()`), so we cannot trust the `Box` pointer; we align
by hand.

Do the arithmetic. `top` is 16-aligned. After the six pops, `rsp = saved_sp + 48 = top - 8`.
`ret` pops the return slot, leaving `rsp = top` — exactly 16-aligned — on entry to
`task_trampoline`. That is precisely the alignment a `call` inside the trampoline demands.

Get this off by 8 and the failure is spectacular and silent: the first SSE instruction the
compiler emits against a misaligned stack (a `movaps`, say) raises `#GP`, which with no
recovery escalates to a triple fault, which on QEMU is an instant reboot with no message.
This is the single most likely bug in the whole milestone, so it was verified under GDB
during commit 1: breaking at `task_trampoline` on the first switch showed **`rsp =
0x40008ff0`** — a 16-aligned value (`0xff0` ends in `0`), proving the fabricated frame
lands the task on a legal stack. That GDB check, not a hopeful recompile, is how a
triple-fault-class bug gets pinned; see the plan's GDB note.

## task_trampoline: where a fresh task begins, and ends

`task_trampoline` is the shim every fresh task first returns into. It is tiny:

```asm
task_trampoline:
    call r15           ; r15 was fabricated to hold the entry fn pointer
    call task_exit     ; the body returned: retire this task; does not return
.hang:
    cli
    hlt
    jmp .hang
```

`call r15` runs the task's entry function — the pointer we fabricated into the `r15`
slot. When the body *returns* (worker tasks are bounded, so they do), we hit a problem:
there is nothing valid on the stack below the fabricated frame to `ret` into. A `ret`
here would jump into garbage. So instead the trampoline falls into `call task_exit`, the
Rust function that marks this task `Finished` and yields away forever. Because a finished
task is never requeued, `yield_now` will never switch back to it, and `task_exit` never
returns. The `cli; hlt` loop below is an unreachable safety net — belt and braces for the
case that should be impossible.

`task_exit` (in `src/task.rs`) is careful about one thing: it disables interrupts *for
good* before touching the scheduler lock, and never restores them. A task that is retiring
must not be preempted while holding a lock — the serial lock during its farewell message,
or the scheduler lock — because a task parked forever mid-lock would wedge every other
task. The task we switch *to* restores its own interrupt flag.

One deliberate omission is worth calling out now because it becomes the crux of preemption:
`task_trampoline` does **not** execute `sti`. It leaves interrupts exactly as the `ret`
that reached it left them — off. We will see in a moment why that neutrality is correct and
why each preemptible task turns its own interrupts on.

## The round-robin scheduler

One switch primitive plus a policy makes a scheduler. The policy here is the simplest fair
one — **round-robin** — and it lives in `Scheduler`:

```rust
struct Scheduler {
    tasks: Vec<Task>,        // every task, indexed by id
    ready: VecDeque<usize>,  // indices of runnable non-idle tasks, FIFO
    current: usize,          // the running task
}
```

`ready` is a FIFO queue of task indices. `yield_now` pops the front (the task waiting
longest), makes it current, and pushes the outgoing task onto the back — so everyone takes
turns in order. Both `Vec` and `VecDeque` come straight from the M8 heap; this is exactly
the "runtime list of tasks" [heap.md](heap.md) predicted the scheduler would need.

**The idle task.** There is a subtlety in "run the next ready task": what if none are ready?
`kernel_main` itself is registered as task 0, the **idle task**, via `Task::idle()`. It is
special in two ways. It owns no fabricated stack (`saved_sp: 0`, an empty `_stack`) because
it is *already running* on the boot stack — its real `saved_sp` gets filled in by the first
switch *away* from it. And it is deliberately kept **out** of the `ready` queue: it is the
fallback, not a peer. When `yield_now` finds the queue empty it falls back to `IDLE`:

```rust
let next = sched.ready.pop_front().unwrap_or(IDLE);
```

So when every worker has finished, control returns to `kernel_main`, which is where the
self-test picks up its result. The idle task is the floor the scheduler can always stand on.

### The lock-drop-before-switch discipline

`SCHED` is a `spin::Mutex<Option<Scheduler>>` — the house-style lock. But it carries one
iron rule that the ordinary mutex users don't: **the guard is dropped before every
`switch_context`.** Look at the shape of `yield_now`:

```rust
let switch: Option<(*mut u64, u64)> = {
    let mut guard = SCHED.lock();
    let sched = guard.as_mut().expect(...);
    // ... pick old and next, update states and the ready queue ...
    let old_sp_ptr = &mut sched.tasks[old].saved_sp as *mut u64;
    let new_sp = sched.tasks[next].saved_sp;
    Some((old_sp_ptr, new_sp))
};  // guard dropped HERE — before the switch

if let Some((old_sp_ptr, new_sp)) = switch {
    unsafe { switch_context(old_sp_ptr, new_sp) };
}
```

Everything the switch needs — a raw pointer to the outgoing task's `saved_sp`, and the
incoming task's `saved_sp` value — is extracted *while the lock is held*, and then the
guard is dropped *before* `switch_context` runs. Why it must be this way: if you held the
lock across the switch, the instant the resumed task called back into the scheduler and
tried to `SCHED.lock()`, it would spin forever on a lock the parked task still holds — and
on one core the parked task can never run to release it. Deadlock. This is the concrete
answer to the "revisit re-entrancy at M9" note the frame, paging, and heap modules all
deferred: the fix is to make the critical section end *before* the switch begins.

## Time: the PIT

Cooperative scheduling has an obvious flaw: a task that never calls `yield_now` keeps the
CPU forever. To take control back *against a task's will*, the kernel needs an outside
event that fires on a schedule it doesn't ask permission for — a clock. That is the
**Programmable Interval Timer** (the 8253/8254), the kernel's first sense of time.

The PIT is a tiny chip that counts down from a value you choose, at a fixed input frequency
of **1,193,182 Hz** (`PIT_FREQ_HZ` in `src/pit.rs` — a third of the old NTSC colourburst
crystal, a fossil of PC history). Each time the count hits zero it raises **IRQ0**, reloads,
and counts down again. You don't set the interrupt *rate* directly; you set the **divisor**
— the reload value — and the rate falls out as `PIT_FREQ_HZ / divisor`. We want 100 Hz (a
tick every 10 ms), so:

```text
divisor = 1_193_182 / 100 = 11931   (integer division)
actual  = 1_193_182 / 11931 ≈ 100.006 Hz   (exact to well under a percent)
```

Programming it is three `outb`s in a fixed order, from `pit::init`:

```rust
outb(PIT_COMMAND, 0x34);                  // command register (port 0x43)
outb(PIT_CH0_DATA, (divisor & 0xff) as u8);  // low byte first  (port 0x40)
outb(PIT_CH0_DATA, (divisor >> 8) as u8);    // high byte second
```

The command byte **0x34** decodes as: **channel 0** (the one wired to IRQ0), **lo/hi-byte
access** (we'll write the divisor as two bytes, low then high), **mode 2** (rate generator),
and binary counting. **Mode 2** is the honest "fire once per period" model — one clean pulse
each time the counter expires, then automatic reload, forever. That steady heartbeat is
exactly what preemptive scheduling rides. Mode 2's access order *requires* the low byte
before the high byte; write them reversed and you set a wildly wrong period — which is why
the two `outb`s to `PIT_CH0_DATA` are ordered and commented.

A `static TICKS: AtomicU64` counts ticks since the timer came online. It is an atomic rather
than a lock for the same reason the keyboard's shift state is: it is touched from an
interrupt handler, which must never risk blocking on a lock. `handle_interrupt` does a single
relaxed `fetch_add`; `ticks()` reads it back as the kernel's coarse clock.

**Why the PIT and not the APIC timer?** Because the PIT rides machinery that already exists.
IRQ0 goes through the same 8259 PIC, the same remap, the same `send_eoi`, and the same IDT
stub the keyboard set up in M5 — about fifteen new lines total. The local APIC timer would be
a milestone of its own: enabling the LAPIC, MMIO at `0xFEE00000`, calibrating against another
clock, and moving off the 8259s — with essentially *zero* extra learning about scheduling.
That is a named future milestone, deliberately held out of this one.

## IRQ0 wiring: the tick becomes an interrupt

The timer is IRQ0, and the PIC remaps IRQ0 to **vector 0x20** (`PIC1_OFFSET`). Two small
edits connect it:

- **`src/pic.rs` unmasks IRQ0.** The master mask byte becomes `0xff & !((1<<1) | (1<<0)) =
  0xFC`, unmasking both the keyboard (IRQ1) *and* now the timer (IRQ0). Every other line stays
  masked.
- **`src/interrupts.rs` gains a dispatch arm** at `TIMER_VECTOR` (`= PIC1_OFFSET`, i.e. 0x20),
  sitting right beside the keyboard's:

```rust
TIMER_VECTOR => {
    crate::pit::handle_interrupt();  // count the tick
    pic::send_eoi(0);                // acknowledge the PIC — BEFORE the switch
    crate::task::preempt();          // hand the CPU to the next task
}
```

**The order is load-bearing, and the EOI-before-switch is the classic first-timer trap.**
When the PIC delivers an interrupt it marks that line "in service" and will not deliver
another until you send the **End Of Interrupt**. If you switch tasks *before* the EOI, the
outgoing task's tick is never acknowledged — and because `preempt` doesn't return until this
task is scheduled again (possibly much later, possibly never), the EOI is stranded. The
symptom is unmistakable and exactly the one [keyboard-and-pic.md](keyboard-and-pic.md) warned
about: **the clock fires exactly once, then goes silent forever.** It looks like a hang, not
a fault. Sending `send_eoi(0)` first, while we're still on the outgoing task, keeps the
heartbeat alive across the switch.

One more placement detail: this arm must sit *above* the spurious-interrupt guard that
catches unhandled vectors in the `0x20..0x2F` range. Put it below and IRQ0 would be swallowed
as spurious and never preempt anything.

## Preemption: the timer forces the switch

Now the two halves meet. A timer tick arrives, the dispatch arm calls `task::preempt`, and
`preempt` does the same thing a cooperative task does voluntarily — except the running task
never asked for it. The elegant part is how little new code this takes. `preempt` is, in full:

```rust
pub fn preempt() {
    yield_now();
}
```

**Approach B: one switch, both paths.** Preemption reuses `switch_context` *verbatim* — there
is no second, interrupt-frame-based switch implementation. This works because `switch_context`
is an ordinary `extern "C"` call. When the timer fires, `isr_common` has *already* saved all
fifteen general-purpose registers into an `InterruptContext` on the interrupted task's stack
(that is what every interrupt does — see [interrupts.md](interrupts.md)). So the caller-saved
registers are already preserved on the stack, and `switch_context` only has to preserve the
callee-saved six of `interrupt_dispatch`'s own frame. When this task is later resumed, control
unwinds back up through `preempt` → `interrupt_dispatch` → `isr_common`'s register restore →
`iretq`, which pops the `InterruptContext` and resumes the interrupted instruction with the
full register state — *and the saved RFLAGS* — restored. The single decision to reuse one
switch is what keeps preemption small.

### The key subtlety: interrupt-safe locking

Here is the part that will bite you if you don't see it coming. The scheduler lock is now
reachable from *two* directions: a task calling `yield_now` voluntarily, and the timer IRQ
calling `preempt`. Picture the disaster on one core: a task calls `yield_now`, takes
`SCHED.lock()`, and — mid critical section, lock held — a timer tick fires. The interrupt
runs `preempt` → `yield_now`, which tries to take `SCHED.lock()` *again*. It spins. But the
only code that could release the lock is the task it just interrupted, which cannot run until
the interrupt returns. Deadlock, on a single core, from a lock that looks perfectly innocent.

**The rule that prevents it: every acquisition of `SCHED` must run with interrupts disabled
(IF=0).** If a timer tick can't fire while the lock is held, it can't re-enter and self-
deadlock. This is why `init`, `spawn`, and `yield_now` all bracket their lock with
`interrupts::save_and_disable()` / `interrupts::restore()`.

`yield_now` is the interesting one, because it clears IF across *both* the critical section
*and* the switch:

```rust
pub fn yield_now() {
    let flags = interrupts::save_and_disable();   // IF = 0 for the whole window
    let switch = { /* lock, pick, extract, drop guard */ };
    if let Some((old_sp_ptr, new_sp)) = switch {
        unsafe { switch_context(old_sp_ptr, new_sp) };
    }
    interrupts::restore(flags);                   // restore IF — for THIS task
}
```

The elegance is in what "restore" means to different tasks. `save_and_disable` returns the
prior IF and `restore` re-enables interrupts *only if they were on before* — it never turns
on interrupts that were off. So each task carries its own interrupt flag through the switch:
the value saved at the top is restored at the bottom *when this particular task is next
resumed here*. A task that yielded with interrupts on gets them back on; the idle task,
yielding with them off, gets them back off. Meanwhile a task that was *preempted* (not
cooperative) doesn't rely on this `restore` at all — its true RFLAGS was saved by the CPU
into its `InterruptContext` and is restored later by `iretq`. Two resume paths, each
restoring the right interrupt state for how the task was suspended.

### Why a fresh task must enable its own interrupts

This is the lesson `task_trampoline`'s missing `sti` was setting up. **A task must have
interrupts enabled to be preempted** — the timer can only steal the CPU from a task whose IF
is set. But a freshly-bootstrapped task is resumed via the cooperative `ret` path, which ran
with IF cleared, and the trampoline deliberately does *not* `sti`. So a brand-new task starts
with interrupts *off* and, left alone, could never be preempted. The fix is for each
preemptible task to enable its own interrupts as its first action:

```rust
extern "C" fn preempt_waiter() {
    interrupts::enable();                 // a task must turn its own interrupts ON
    while !PREEMPT_FLAG.load(Ordering::Acquire) {
        core::hint::spin_loop();          // NO yield — only the timer can break us out
    }
    WAITER_SAW_FLAG.store(true, Ordering::Release);
}
```

Why not just `sti` in the trampoline and be done? Because the trampoline serves *both* the
cooperative self-test (whose tasks run fully masked, before the PIC is even configured) and
preemptible tasks. A single unconditional `sti` there would force interrupts on for the
cooperative tasks too, which is wrong. Keeping the trampoline neutral and making each task own
its interrupt policy is what lets one shim serve both worlds — and it makes "a task must have
interrupts on to be preempted" an explicit, visible line of code rather than a hidden
assumption.

## Verification honesty: working vs. accidentally working

The sibling docs are all careful about the gap between something that *works* and something
that only *looks* like it works, and scheduling has the sharpest version of the trap:
a broken scheduler can produce perfectly plausible output while quietly corrupting state or
while not actually being a scheduler at all. The two self-tests in `src/task.rs` are each
built to be *un-fakeable* by the failure they target.

**The cooperative proof: strict alternation plus a carried checksum.** `cooperative_self_test`
spawns two workers that each append their letter to a shared `String` and then `yield_now`.
If the switch really preserves and resumes each call stack, the letters must come out in
strict `ABABABAB` order — and the test asserts exactly that:

```rust
assert_eq!(seq, expected, "cooperative scheduler: tasks did not alternate cleanly ...");
```

But clean alternation alone could hide a switch that scrambles register state, so each worker
also carries two things *across every yield* and re-checks them on resume: a running checksum
`acc`, and a heap `Box::new(acc)`:

```rust
let carried = Box::new(acc);
yield_now();
assert_eq!(*carried, acc, "task: heap value corrupted across a context switch");
```

`acc` living in a callee-saved register across the yield proves the switch preserved
callee-saved state; the `Box` surviving proves heap allocation interleaved with switching
doesn't corrupt anything. Both workers must end at the same independently-computed checksum
(`fold_checksum`), asserted at the end. A switch that dropped a register or mangled the stack
would fail one of these on the spot, not produce clean letters.

**The preemptive proof: completion *is* the proof.** The danger with preemption is subtler —
a scheduler that's secretly still cooperative would pass every cooperative test. So
`preemptive_self_test` spawns two tasks that **never call `yield_now` at all.** A "waiter"
spins on an `AtomicBool` flag; a "setter" does a bounded chunk of work and then flips it. The
waiter can only escape its loop if the flag becomes true — and only the setter can set it —
and neither task ever yields. **The single thing that can hand the CPU from one to the other is
the timer interrupt.** If preemption were broken, the waiter would spin forever and the boot
test would hang. So merely *reaching the end* proves the timer preempts:

```rust
assert!(WAITER_SAW_FLAG.load(Ordering::Acquire),
        "preemption failed: the non-yielding waiter never saw the setter run");
```

This is the deterministic analogue of M8's reclaim test: it is the one thing a
cooperative-only scheduler provably *cannot* do, so passing it can't happen by accident. Note
the honesty in the failure mode — a broken build here **hangs**, it does not print a false
success. The proof is completion, not a claim.

The actual serial output when it all works:

```text
[ok] scheduler: 2 tasks alternated cleanly (ABABABAB) over 4 switches each; ...
M9: cooperative scheduler online
[ok] timer: PIT channel 0 at ~100 Hz (divisor 11931)
[ok] scheduler: preemption works -- a task that never yields was interrupted by
     the timer and another task ran (95 ticks elapsed)
M9: preemptive scheduler online
```

That "95 ticks elapsed" is the timer's own heartbeat, counted independently of the scheduler
— cross-checkable against QEMU's `-d int` log, which shows repeated `v=20` entries at ~100 Hz.

## The honest limitations

Following the discipline the sibling docs insisted on — name what the design *doesn't* do, out
loud — this scheduler has four known shortcomings, each a deliberate deferral with a named path
forward:

- **No guard page on task stacks.** A `Task`'s stack is a plain `Box<[u8]>` from the heap, which
  has no guard pages. A task that overflows its 16 KiB stack writes *downward* past the
  allocation into an adjacent heap object and faults **nothing** — silent corruption, not a
  clean trap. This is tight under preemption specifically: a timer IRQ pushes a full
  `InterruptContext` (~136 bytes) plus the `interrupt_dispatch` → `preempt` → `yield_now` →
  `switch_context` call chain onto whichever task stack is running, on top of the task's own use.
  A real guard page needs an unmapped page below each stack (per-stack M7 paging) — a future
  refinement, and the concrete motivation for the deferred TSS/IST work below.
- **QEMU's PIT is approximate.** QEMU does not emulate the 1.193 MHz crystal cycle-accurately;
  ticks jitter and can coalesce, especially across `hlt`. The *rate* is right to a few percent
  over a second, which is all a tick counter needs — but do not treat inter-tick spacing as
  precise. Precision is the later APIC/TSC milestone's problem.
- **Single core, round-robin, no priorities.** One CPU, one ready queue, FIFO, no sleep/block/
  join/IPC. That is a real scheduler's job; this is two toy tasks and a queue that *happens* to
  generalize to N. The interrupt-safe locking above is exactly right for one core and would need
  rethinking (real per-CPU locks) the day a second core appears — which is not a goal.
- **Soft-float means MXCSR and the x87 control word are not saved.** `switch_context` carries only
  the six integer callee-saved registers because SSE/MMX/x87 are disabled on this target (see
  `.cargo/config.toml`). If floating point were ever turned on, `MXCSR` and the x87 control word
  are *also* callee-saved, and a switch that ran FP code would have to carry them too. The switch
  is correct precisely *because* of a build-time choice made elsewhere — worth knowing before you
  flip on SSE and wonder why numbers drift across a preemption.

None of these corrupt memory in normal operation with a handful of small tasks; all are the
natural next refinements.

## Where this goes next

The scheduler is the first piece of the kernel whose customers are *features*, not more plumbing.
Everything that needs to do more than one thing at a time now has a mechanism to stand on:

- **Milestone 10 (the shell)** is the first real tenant. The characters M5 echoes will feed a line
  buffer that runs as a task, scheduled on this very machinery — "it listens" finally becomes "it
  runs a program while the timer ticks underneath."
- **A real APIC timer** replaces the PIT when precision and per-CPU timing matter: enable the
  LAPIC, MMIO at `0xFEE00000`, calibrate, and retire the 8259 path. Named and deferred here so it
  can't leak back in.
- **TSS + IST, and guard pages** give the double-fault (and eventually each task stack) its own
  known-good stack, so a stack overflow *reports* instead of silently corrupting a neighbor — the
  robustness upgrade [interrupts.md](interrupts.md) first flagged, now with a concrete reason to
  want it. The `ist` field in every IDT entry is already present, wired to 0, waiting.

That is the shape of this milestone: teaching one CPU to hold more than one thread of control — by
learning that a paused computation is *just a saved stack pointer*, that a `ret` can resume someone
else's call stack, and that a timer tick is all it takes to make the swap happen whether the running
task consents or not.
