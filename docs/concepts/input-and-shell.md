# Input and the shell, from first principles

*Companion to Milestone 10. Read this before `src/keyboard.rs`, `src/shell.rs`,
and the keyboard arm of `src/interrupts.rs` — it explains what those files are
*for* and why "just write a shell" hides the real lesson, which is not the shell
at all but the **input path**: how a keystroke crosses from a hardware interrupt,
through a queue, into a running program. `src/keyboard.rs` gains a lock-free
single-producer/single-consumer ring (`RING`/`HEAD`/`TAIL`, `push`/`pop`);
`src/interrupts.rs` rewires the keyboard IRQ from *echo* to *enqueue*;
`src/shell.rs` holds the shell itself — a `Line` editor, a `Command` parser, and
`shell_main`, the task that drains the ring. This builds directly on three
siblings: [timer-and-scheduling.md](timer-and-scheduling.md) — the shell is an
ordinary scheduler task, spawned on M9's `switch_context`/`preempt` machinery,
so read that first if "task" or "yield" is new; [keyboard-and-pic.md](keyboard-and-pic.md)
— the IRQ1 → PIC → EOI path this milestone rewires; and [heap.md](heap.md) —
whose `Vec`/`String` the shell parses into, and whose lock is the subject of the
milestone's trickiest invariant. The one thing you need coming in: after the
earlier milestones we are in long mode, the heap is up, and the scheduler and the
keyboard IRQ are both live — the timer ticks at 100 Hz and `sti` has already run.*

---

## The problem: the kernel can compute, but it can't be *told* anything

Look at what the kernel can already do. It manages physical frames and page
tables. It hands out heap memory, so `Vec` and `String` work. It runs several
threads of control on one CPU and preempts them 100 times a second whether they
consent or not. It is, by any fair measure, an operating system — and it is
completely deaf. Nothing you do at the keyboard changes what it does. Since M5 it
has *echoed* keystrokes to the screen, which looks like listening, but echoing is
a reflex, not comprehension: the character bounces off the interrupt handler
straight onto the VGA buffer and is gone. No program ever *received* it. The
kernel can compute and it can preempt, but it cannot be *told* anything.

This milestone closes that gap, and the gap is smaller and stranger than it
looks. The obvious framing — "write a shell" — points at the wrong thing. A
shell is a read-eval-print loop, and REPLs are easy; the hard, general question
underneath it is:

> **How does a keystroke — an event that arrives at a random instant, inside an
> interrupt handler that must not do any real work — reach a *program* that can
> take its time deciding what the keystroke means?**

Answer that and the shell falls out almost for free. Get it wrong and you get a
kernel that deadlocks the first time someone types while the timer fires. So this
doc is named for the lesson, not the deliverable: **input** first, **shell**
second. The shape of the answer is one sentence, and everything below is that
sentence unpacked:

> **"Interactive" is a task polling a queue that an interrupt fills.** The IRQ is
> a *producer* — it drops one byte in a queue and gets out. The shell task is a
> *consumer* — it drains the queue on its own schedule, echoes, edits a line, and
> runs commands. The queue is the seam between them.

## Why M5's echo was a latent deadlock

Start with what we are moving *away* from, because the old design was not just
limited — it was quietly unsafe, and seeing why motivates the whole rewrite.

In M5 the keyboard IRQ did the work itself. Its dispatch arm read the scancode,
translated it, and **printed it** — right there, inside the handler:

```text
you press 'k'
  -> IRQ1 -> interrupt_dispatch
       keyboard::handle_interrupt() reads 0x60, returns 'k'
       print!("k")            <-- the handler takes the VGA writer's lock
       pic::send_eoi(1)
  -> iretq
```

That `print!` takes the VGA writer's spinlock. [keyboard-and-pic.md](keyboard-and-pic.md)
argued this was *safe in M5* — and it was, then, because M5's IDT entries are
interrupt gates (IF clears on entry, so the handler can't interrupt itself) and
the main loop never printed with interrupts on, so the lock was always free when a
keystroke arrived. The doc was careful to add the caveat: "If the main loop ever
printed while interrupts were on, the hazard would return."

M9 is exactly the world that caveat warned about. Now there are *tasks* running
with interrupts enabled, and they print. Picture it: a task calls `print!`, takes
the VGA lock, and — one instruction into the critical section, lock held — a
keystroke fires IRQ1. The handler runs, reaches its own `print!`, and spins on the
VGA lock. But the only code that can release that lock is the task the interrupt
just froze, and on one core that task cannot run until the handler returns.
Deadlock — silent, total, from a lock that looked innocent for four milestones.

The fix is not "be careful with the lock." The fix is to make the handler stop
doing work that could ever block. **An interrupt handler must never touch a lock
another context might hold.** So the rewired keyboard arm does the one thing that
can't block — it stores a byte into a lock-free queue — and hands *all* the real
work (echoing, editing, dispatching) to a task that runs later, with interrupts
on, holding no dangerous lock. The comment in `interrupts.rs` says it outright:
the refactor "also removes a latent deadlock: the handler no longer takes the VGA
writer lock, so it can never spin on a lock a preempted task is holding
mid-print." The producer/consumer split isn't just a nicer architecture; it is
the deadlock's cure.

## The lock-free SPSC ring

The queue between producer and consumer is a **single-producer, single-consumer
(SPSC) ring buffer** of bytes, and it lives in `src/keyboard.rs`. It is worth
slowing down on *why it needs no lock at all*, because that is the property that
makes it safe to touch from an interrupt handler.

```rust
const RING_CAP: usize = 256;
static RING: [AtomicU8; RING_CAP] = [const { AtomicU8::new(0) }; RING_CAP];
static HEAD: AtomicUsize = AtomicUsize::new(0); // consumer cursor — only the shell advances it
static TAIL: AtomicUsize = AtomicUsize::new(0); // producer cursor — only the keyboard IRQ advances it
```

A ring is a fixed array with two cursors chasing each other around it, wrapping at
the end. `TAIL` is where the next byte gets *written*; `HEAD` is where the next
byte gets *read*. The producer advances `TAIL`, the consumer advances `HEAD`, and
when they meet the ring is empty.

```text
        push (IRQ, producer)                    pop (shell, consumer)
        advances TAIL ─┐                     ┌─ advances HEAD
                       ▼                     ▼
        ┌────┬────┬────┬────┬────┬────┬────┬────┬────┬── ... ──┐
  RING: │    │    │ k  │ e  │ y  │    │    │    │    │         │  (256 slots)
        └────┴────┴────┴────┴────┴────┴────┴────┴────┴── ... ──┘
                  ▲              ▲
                 HEAD           TAIL
                (read here)    (write here)
              unread bytes: HEAD..TAIL   |   the IRQ fills ahead, the shell drains behind
```

**Why no lock is needed.** A lock exists to stop two writers from clobbering the
same location. Here, by construction, *each index has exactly one writer*: only
the keyboard IRQ ever writes `TAIL`, only the shell task ever writes `HEAD`. There
is no location two parties both mutate, so there is nothing for a lock to protect.
That is the whole reason the SPSC discipline matters — it is what lets the queue
be lock-free, which is what lets the producer live in an interrupt handler where
blocking is forbidden. This is the house rule made concrete, the same one that put
the keyboard's shift state and the PIT's tick count in atomics rather than locks:
*state touched by an interrupt uses atomics, never a lock.*

But "no lock" is not "no rules." The two parties still have to agree on *ordering*
— specifically, the consumer must never read a slot before the producer has
finished writing it. That agreement is the **Acquire/Release contract**, and it is
the one real correctness precondition of the whole ring.

```rust
pub fn push(byte: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % RING_CAP;
    if next == HEAD.load(Ordering::Acquire) {
        return; // full: drop the newest keystroke rather than overwrite unread data
    }
    RING[tail].store(byte, Ordering::Relaxed);   // (1) write the byte
    TAIL.store(next, Ordering::Release);         // (2) THEN publish the new tail
}

pub fn pop() -> Option<u8> {
    let head = HEAD.load(Ordering::Relaxed);
    if head == TAIL.load(Ordering::Acquire) {    // (A) read the tail first
        return None; // empty
    }
    let byte = RING[head].load(Ordering::Relaxed); // (B) THEN read the byte
    HEAD.store((head + 1) % RING_CAP, Ordering::Release);
    Some(byte)
}
```

Read the ordering as a promise between the two sides:

- **The producer publishes the byte *before* the tail.** `push` writes `RING[tail]`
  and *then* stores `TAIL` with `Release`. The `Release` is a one-way fence: every
  write before it (the byte) is guaranteed visible to anyone who reads that store.
- **The consumer reads the tail *before* the byte.** `pop` loads `TAIL` with
  `Acquire` and *then* reads `RING[head]`. The `Acquire` pairs with the producer's
  `Release`: if `pop` sees the advanced tail, it is guaranteed to also see the byte
  the producer wrote before advancing it. So the consumer can never observe a slot
  the producer hasn't finished filling.

The bytes themselves use `Relaxed` because the index ordering already fences them —
the `TAIL` store/load pair is what carries the happens-before edge; the byte just
rides along.

**Be precise about what this buys on one core.** Ziran runs on a single CPU. There
is no second core to *race* — the IRQ and the shell never truly execute at the same
instant; an interrupt is a detour the one core takes and returns from. So the
Acquire/Release pair is not defending against a hardware memory-reordering race
between two cores. What it *is* defending against is the **compiler** reordering the
byte-write past the tail-store within a single stream of instructions. `Relaxed`
would let the optimizer float `TAIL.store` above `RING[tail].store`, and then a
keystroke IRQ landing between them would publish a tail that points at a stale slot.
Acquire/Release forbids that reordering. It states the contract correctly regardless
of core count — which is the honest way to write it — but on this machine its teeth
are on the compiler, not a second CPU.

### Full-vs-empty, and dropping the *newest*

Two design decisions in that small code carry more weight than they look.

**One slot is always reserved.** The ring holds `RING_CAP = 256` slots but only
**255 usable bytes**. Why sacrifice one? Because `HEAD == TAIL` has to mean
*something unambiguous*, and it is the natural encoding of "empty." If the producer
were allowed to fill all 256 slots, a completely full ring would *also* have
`HEAD == TAIL` (the tail having wrapped all the way around), and the two states
would be indistinguishable. So `push` refuses to advance into the slot right behind
`HEAD`: it computes `next = (tail + 1) % RING_CAP` and bails if `next == HEAD`.
Full is "one slot short of catching HEAD"; empty is "HEAD equals TAIL." One
reserved slot keeps the two apart with no extra flag. 255 usable bytes is still far
more backlog than a human typist can build against a shell that drains every timer
tick — the reserved slot costs nothing real.

**On a full ring, drop the *newest* byte, not the oldest.** When `push` finds the
ring full it simply `return`s, discarding the byte it was handed. It does *not*
overwrite the oldest unread byte to make room. This looks like a coin-flip choice;
it is not. Overwriting the oldest byte would mean the producer writing at `HEAD` —
and `HEAD` is the *consumer's* index, the one the consumer is simultaneously reading
and advancing. The instant the producer writes where the consumer reads, the
single-writer-per-index invariant is broken, and with it the entire reason the ring
needs no lock. Dropping the newest keystroke keeps the producer confined to `TAIL`,
where it is the only writer. The overflow is bounded and honest — a keystroke lost
under a 255-byte backlog is a non-event — and it preserves the invariant that makes
the whole structure sound. The safe choice and the correct choice are the same
choice.

## The shell is a scheduler task

Now the consumer. The shell is not special machinery — it is an ordinary task on
the M9 scheduler, spawned exactly like the self-test workers were:

```rust
task::init();                 // fresh scheduler (the self-tests left it full of Finished tasks)
task::spawn(shell::shell_main);
task::yield_now();            // hand the CPU to the shell; kernel_main becomes idle
```

`shell_main` is an `extern "C" fn()` matching `Task::new`'s signature, and it never
returns — it is a `loop` forever, so `task_exit` never fires for it. That is the
milestone's central claim in one line: *interactivity is not faked inline in
`kernel_main`; it is a real scheduled task.* You can prove it by typing `ps` and
seeing the shell listed alongside the idle task. If the shell weren't a task, `ps`
couldn't show it — the milestone would be lying about its own architecture.

Three details of how the task lives are load-bearing.

**It enables its own interrupts.** A freshly spawned task is reached through
`switch_context`'s `ret`, not through `iretq`, and that `ret` path ran with IF
cleared — so *every* task starts with interrupts off (this is the missing-`sti`
subtlety from [timer-and-scheduling.md](timer-and-scheduling.md)). The shell's very
first action must therefore be to turn its own interrupts on:

```rust
pub extern "C" fn shell_main() {
    interrupts::enable();     // a task starts IF=0; without this the hlt below wedges forever
    // ... banner, prompt, then the REPL ...
}
```

Skip this line and the failure is silent and total: the shell reaches its `hlt`
(below) with interrupts disabled, and `hlt` with IF=0 is a *permanent* halt — no
interrupt can ever wake the CPU, so no keystroke IRQ ever arrives, so the shell
never runs again. The one line `interrupts::enable()` is the difference between a
live prompt and a dead machine.

**When the ring is empty, it `hlt`s.** The REPL's structure is a poll:

```rust
let mut line = Line::new();
loop {
    match keyboard::pop() {
        Some(byte) => { /* echo / edit / dispatch */ }
        None => unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        },
    }
}
```

Why `hlt`, specifically? Consider the two alternatives and why each is wrong:

- **A bare spin** (`loop { pop() }` with no sleep) would peg the core at 100% CPU,
  burning power to ask "any byte yet?" millions of times a second. That is the
  opposite of the interrupt-driven idle [keyboard-and-pic.md](keyboard-and-pic.md)
  was proud of.
- **`yield_now`** would seem right — "nothing to do, let another task run" — but
  with only the shell and the idle task in existence, the ready queue is empty, so
  `yield_now` bounces to idle, idle immediately yields back, and the two ping-pong
  every scheduling opportunity, again pegging the core doing nothing. Worse,
  `yield_now` when the shell is the only ready task hits the `next == old` no-op
  path and doesn't even switch.

`hlt` is the honest idle: it sleeps the CPU until the *next interrupt*, drawing no
power in between. What wakes it depends on who is current. If the shell is the
current task, its own keystroke IRQ fires, the handler pushes the byte and returns,
and `hlt` falls through to re-poll — instant. If a timer tick has meanwhile parked
the shell and made idle current, the byte simply waits in the ring until the next
tick preempts idle and reschedules the shell — a latency bounded by the 100 Hz
period, never a lost byte. Either way the `hlt` is safe *only* because the shell
holds no lock across it and runs with IF=1; the lock-free ring is what guarantees
there is no lock to strand.

**`kernel_main` becomes the idle task.** After `yield_now` hands control to the
shell, `kernel_main` falls into `hlt_loop()` — it *is* task 0, the idle task from
M9, resumed only when nothing else is runnable. The kernel's original thread of
control has demoted itself to the floor the scheduler stands on, and the shell is
now the tenant doing the real work. That inversion — the boot path becoming the
idler, a spawned task becoming the driver — is what "running a program" finally
means here.

## Line editing and command dispatch: pure logic behind the I/O

Inside the REPL, the actual work splits into two pure pieces and a thin I/O shell
around them. This split is not tidiness for its own sake — it is what makes the
whole thing *testable without a keyboard*, which is the verification story below.

**Editing is pure: `Line::edit` returns an `Edit`, does no I/O.** The line buffer
is heapless and fixed — `struct Line { buf: [u8; LINE_MAX], len: usize }` with
`LINE_MAX = 128`. Folding one byte into it is a pure function that *reports* what
happened rather than *doing* it:

```rust
enum Edit { Echo(u8), Erase, Submit, Ignored }

fn edit(&mut self, byte: u8) -> Edit {
    match byte {
        b'\n' => Edit::Submit,
        0x08 => if self.len > 0 { self.len -= 1; Edit::Erase } else { Edit::Ignored },
        0x20..=0x7e => if self.len < LINE_MAX {
            self.buf[self.len] = byte; self.len += 1; Edit::Echo(byte)
        } else { Edit::Ignored },
        _ => Edit::Ignored,
    }
}
```

`edit` never prints. It mutates the buffer and returns an outcome; the REPL decides
what that outcome *looks like* (`Echo(b)` → print the char, `Erase` → print `\u{8}`,
`Submit` → dispatch the line). The edge cases live here and are all in the return
type: a printable byte past `LINE_MAX` is `Ignored` (no overflow, no wrap); a
backspace on an empty line is `Ignored` (`len` never underflows, the cursor never
walks back over the prompt); a stray control byte is `Ignored`. Because `edit` is
pure, every one of those cases is an assertion in the self-test, not a thing you
have to type by hand.

**Parsing is pure: `parse` returns a `Command`, borrows the line.** Command
*recognition* is likewise factored out of command *execution*:

```rust
enum Command<'a> { Empty, Help, Echo(&'a str), Clear, Mem, Ps, Unknown(&'a str) }

fn parse(line: &str) -> Command<'_> {
    let line = line.trim();
    if line.is_empty() { return Command::Empty; }
    let (verb, rest) = match line.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim_start()),
        None => (line, ""),
    };
    match verb {
        "help" => Command::Help, "echo" => Command::Echo(rest),
        "clear" => Command::Clear, "mem" => Command::Mem, "ps" => Command::Ps,
        other => Command::Unknown(other),
    }
}
```

`parse` touches no hardware and allocates nothing — it borrows the input line and
returns a classification. `dispatch` is the only impure part, the one place that
prints, and it is a flat `match` over the five built-ins: `help` lists them, `echo`
prints its tail verbatim (internal double-spaces preserved, trailing ones not),
`clear` wipes the VGA screen, `mem` and `ps` read live kernel state (next section).
An unknown verb is *reported* — `unknown command: X (try help)` — never silently
swallowed. Five commands, no history, no arrows, no pipes: the scope is deliberately
tiny, because the milestone's depth is in the input path, not the command set.

## `ps` and `mem`: reading live state without deadlocking

`mem` and `ps` are where the shell reaches *into* the kernel's live state, and they
are the milestone's real depth — not because printing a task list is hard, but
because doing it safely under a 100 Hz preempting timer forces two invariants that
are easy to get wrong and fail as *hangs*, not crashes.

**Invariant one: never hold a lock across a `print!`.** This is the deadlock from
the start of the doc, wearing a different hat. Suppose `ps` locked the scheduler and
then printed each task while still holding the lock. Mid-print, the 100 Hz timer
fires, runs `preempt` → `yield_now` → `SCHED.lock()` — and spins forever on the lock
the printing task still holds. The rule that prevents it is **snapshot-then-print**:
copy the state out under the lock, *release the lock*, and only then print. Both
`cmd_ps` (via `task::snapshot`) and `cmd_mem` (via `heap::stats`) obey it. `heap::stats`
walks the free list under the heap lock, sums the free bytes, releases, and *returns*
the total for the caller to print later — no lock is live when `println!` runs.

**Invariant two — the one this milestone added: the snapshot must be *allocation-free*
while holding `SCHED` with interrupts off.** This is subtler, and it is the trap
worth internalizing. Look at `task::snapshot`:

```rust
pub fn snapshot() -> Vec<(u64, State, bool)> {
    const MAX: usize = 64;
    let mut tmp = [(0u64, State::Ready, false); MAX];   // a STACK array, no heap

    let n = {
        let flags = interrupts::save_and_disable();     // IF = 0
        let n = {
            let guard = SCHED.lock();                   // hold SCHED
            let sched = guard.as_ref().expect("...");
            let n = sched.tasks.len().min(MAX);
            for (i, t) in sched.tasks.iter().take(MAX).enumerate() {
                tmp[i] = (t.id, t.state, i == sched.current);  // copy into the stack array
            }
            n
        }; // guard dropped
        interrupts::restore(flags);                     // IF restored
        n
    };

    tmp[..n].to_vec()   // the Vec is built AFTER the lock is released, with interrupts on
}
```

The critical section — IF cleared, `SCHED` held — copies the task table into a
fixed **stack array** `tmp`, and does *not* allocate. The `Vec` is built only
*after* the lock is dropped and interrupts are restored, out in the open where
allocation is safe.

Why does allocating inside that critical section deadlock? Trace it. The heap has
its own lock (M8's `LockedHeap`). Now imagine some *other* task was preempted by the
timer partway through its own `alloc`, still holding the heap lock, and is currently
parked. Meanwhile `snapshot` is running with **IF cleared** — nothing can preempt
it — and it calls `to_vec`, which calls the allocator, which tries to take the heap
lock. It spins. But the task that holds the heap lock can only be rescheduled to
release it *by the timer*, and the timer is masked because `snapshot` cleared IF.
The two conditions — "I hold IF=0 so nothing can run but me" and "I'm blocking on a
lock only something else can release" — are jointly a deadlock. The fix is to keep
the IF=0/`SCHED` window strictly allocation-free: touch only the pre-sized stack
array inside it, and defer the one allocation (`to_vec`) until after IF is restored.

This is the same shape as `heap::stats` (which allocates nothing at all) and the
same discipline `task::spawn` documents but does *not* yet fully satisfy — `spawn`
still allocates a task stack while holding `SCHED` with IF=0, which is safe only
because every current caller spawns during setup, before any allocating task exists
to be preempted mid-`alloc`. `snapshot` is the first accessor to get the invariant
fully right, precisely because it runs at *runtime*, concurrently with a shell that
allocates. Internalize the rule: **allocating under an IF-guarded lock is a latent
deadlock; copy into a stack buffer under the lock, build the heap object after.**

## Verification honesty: working vs. accidentally working

The sibling docs are all careful about the gap between something that *works* and
something that only *looks* like it works, and input has a nasty version of the
trap: you can't type inside an automated boot test, so the tempting move is to
verify nothing and call the manual "I typed and it echoed" a proof. That would
leave the actual hard parts — the ring's ordering, the editing edge cases, the
accessor deadlock — checked only by a human who can't feel a race.

The milestone's insight is the escape hatch: **an interactive program is pure logic
sitting behind an I/O shell.** Because `edit` and `parse` were factored as pure
functions, and the ring's `push`/`pop` are just function calls, almost the entire
milestone can be pinned by a *deterministic* self-test that runs on serial at boot,
with no keyboard involved. `shell::self_test` does exactly that:

- **Editing.** It feeds a synthetic byte stream — `h`, `i`, backspace, `a`, `t` —
  and asserts both the `Edit` outcomes and that the buffer reads `"hat"`. It asserts
  backspace on an empty line is `Ignored` and doesn't underflow, and that Enter
  `Submit`s and the line resets.
- **Parsing.** It asserts every verb classifies correctly, that `echo hello  world`
  preserves its double space, that `echo` with no tail is `Echo("")`, and that a
  bogus verb becomes `Unknown("bogus")` rather than being swallowed.
- **The input ring, the crux.** It round-trips the ring FIFO — pushes `x` then `y`,
  pops them back in order, asserts the ring then reports empty. It does this with
  interrupts disabled (`save_and_disable`) so the real keyboard IRQ — the only other
  producer — cannot push concurrently and break the single-producer contract during
  the test. And it *drains any pending byte first* rather than asserting emptiness,
  because interrupts have been live since `sti` and a key pressed earlier in boot may
  already sit in the ring — an honest touch that keeps a stray keystroke from failing
  a correct test.
- **The `ps`/`mem` accessors.** It calls `task::snapshot()` and asserts the list is
  non-empty and contains the idle task (id 0); it calls `heap::stats()` and asserts
  the free/used figures are in range; it checks free frames exist. The *formatting*
  prints to VGA and is eyeballed, but the data path behind it is asserted on serial.

When it all passes, one line goes out the serial port — the deterministic proof that
everything except the two irreducibly-hardware bits is correct:

```text
[ok] shell: editing, parsing, input ring, and ps/mem accessors verified (tasks=2, heap_free=...)
```

What is left *genuinely* manual is small and already proven elsewhere: the hardware
IRQ actually delivering a byte (the IRQ1 → PIC → EOI path, proven in M5) and VGA
rendering the echo (proven in M4). Under `make run` you type, watch characters echo,
backspace blank a cell, Enter start a fresh prompt, and `ps` list the shell task
beside idle — but every piece of *logic* that could hide a subtle bug was already
nailed down deterministically before a human touched a key. That is the honest
division: pin the logic on serial, hand-check only the two hardware edges that older
milestones already validated.

## The honest limitations

Following the discipline the sibling docs insisted on — name what the design
*doesn't* do, out loud — the input path and shell have five known shortcomings,
each a deliberate deferral with a named path forward:

- **No block/wake; `hlt`-poll instead.** The shell doesn't truly *park* when its
  ring is empty — it `hlt`s and is re-polled by the next interrupt. When the shell
  is current, its own keystroke IRQ wakes it instantly. But when a timer tick has
  parked it and made idle current, a freshly typed byte waits in the ring until the
  next tick reschedules the shell — a worst-case latency of one 100 Hz period, about
  **10 ms**. Imperceptible to a human, but it is a poll, not a wake. Real task
  parking — mark the task blocked, wake it from the keyboard IRQ — needs scheduler
  state (a blocked set, a wake path from an interrupt) this milestone deliberately
  didn't add.
- **`spawn` still allocates under an IF-guarded lock.** `snapshot` got the
  allocation-free invariant right, but `task::spawn` still allocates a task stack and
  may grow its `Vec`/`VecDeque` while holding `SCHED` with IF=0. It is safe *today*
  only because every caller spawns during setup, before any other allocating task
  exists to be preempted mid-`alloc` holding the heap lock. A future milestone that
  spawns tasks at runtime, concurrently with allocating tasks, must make `spawn`
  allocation-free first — the deferral is documented in `spawn`'s own doc comment.
- **The panic-mid-print VGA-lock gap is still open.** The pre-existing hazard
  [interrupts.md](interrupts.md) and the fatal-fault path flagged remains: if a panic
  or fault fires *while* a task holds the VGA writer lock mid-print, the handler's own
  `print!` spins on a lock only the frozen task could release. This milestone
  *removed* the keyboard IRQ from the set of things that can trip it, but the general
  fix — a lock-free emergency writer for the fault path — is still owed.
- **No history, arrows, cursor movement, or pipes.** The line editor is a 128-byte
  fixed buffer with append and backspace, full stop. No up-arrow recall, no
  left/right cursor, no `|` or `>`, no `exit`, no job control. Multi-byte scancodes
  (arrows, function keys) never even reach the ring — `translate` returns `None` for
  them — so there is nothing to handle. This is a REPL for learning, not a userland.
- **`ps` caps at 64 tasks.** `snapshot`'s stack array is `MAX = 64` entries; any task
  beyond that is simply omitted from `ps`. A teaching kernel never has more than a
  handful of tasks, so the cap never bites — but it is a real ceiling, chosen so the
  critical section can use a fixed stack buffer instead of allocating.

None of these corrupt memory in normal operation; all are the natural next
refinements, and the first two are the ones a growing task system will force first.

## Where this goes next

The input path is the kernel's first *inbound* channel — the first time the outside
world can change what the kernel does, rather than just watch it. That opens two
directions the roadmap already names:

- **Milestone 11 (the filesystem)** gives the shell something to operate *on*. The
  parser's flat `match` grows `ls` and `cat`; `Command` gains variants that take
  paths; and the same snapshot-then-print discipline extends to reading directory
  entries and file tables — data structures whose size, like the task list, is known
  only at runtime and lives on the M8 heap. The shell stops being a demo of five
  built-ins and becomes a way to *explore* the system.
- **The teaching tool's live view** (PLAN §3b) is the payoff the whole
  producer/consumer story was building toward: a "type `ps` and watch the tasks
  change" panel that makes the scheduler visible as it runs. Because `ps` already
  reads a true, live snapshot of the scheduler — not a canned string — that live view
  is a rendering problem, not a plumbing one. The honest accessor built here is
  exactly what a real-time visualization needs underneath it.

That is the shape of this milestone: teaching the kernel to *listen* — by learning
that a keystroke reaches a program not because an interrupt does the work, but
because the interrupt drops one byte in a lock-free queue and a scheduled task, on
its own time and holding no dangerous lock, picks it up. **Interactivity is a task
polling a queue an interrupt fills** — and once you see it that way, the shell is
just the first program that bothered to look.
