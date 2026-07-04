# It has a prompt now

*Milestone 10 — a simple shell. The kernel runs its first interactive program: a
prompt that reads what you type and talks back, as a task on the M9 scheduler.*

> New to this? Read
> [docs/concepts/input-and-shell.md](../concepts/input-and-shell.md)
> alongside — it explains how a keystroke travels from a hardware interrupt to a
> program without the interrupt doing any of the work: the producer/consumer
> split, a lock-free ring, and why "interactive" is just a task polling a queue
> that an interrupt fills.

Last milestone gave the kernel *tasks* and preemption: it could run more than one
thread of control on one CPU. This one runs a task that talks to you. The keyboard
has been able to raise interrupts since M5, but until now nothing on the other end
was a *program* — it was a handler echoing characters inline. M10 closes the loop:
a real shell, spawned as an ordinary task on the M9 scheduler, that reads a line,
parses it, and runs one of five built-in commands. The interrupt's job shrinks to
almost nothing; the interesting work moves to a task that shares the CPU with
everything else scheduled.

### Milestone 10 — simple shell — checklist

Think
- [x] /milestone-office-hours — six forcing questions answered below
- [x] /scope-guard — verdict recorded: **Hold**, below

Plan
- [x] /eng-plan — approach, machine-state preconditions, edge cases, verification — [docs/planning/milestone-10-eng-plan.md](../planning/milestone-10-eng-plan.md)

Build
- [x] Working, demoable state reached (live prompt: help/echo/clear/mem/ps, backspace, unknown) — the shell runs as a *spawned task*, visible in its own `ps`
- [x] `make` is clean: assembles, compiles, links; `make check-header` passes
- [x] No new compiler warnings; every new `unsafe`/asm block says why it's sound (the lock-free ring's atomics, the IF-guarded `snapshot`)

Debug (as needed)
- [x] /investigate — not needed; zero runtime debugging cycles (the two real hazards were caught in review, below)
- [x] The real debugging story captured — below (the honest version: nothing broke at runtime)

Review
- [x] /kernel-review — 3-lens pass (input ring, shell/parse logic, locking under the timer); findings fixed

Security
- [x] No new privilege boundary (still ring 0, one address space, no syscall gate — the shell is a kernel task) — /red-team N/A

Reflect
- [x] /retro — post-mortem answered honestly, below
- [x] /document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand.** How a keystroke travels from a hardware
   interrupt to a program *without the interrupt doing the work* — the
   producer/consumer split. The keyboard IRQ doesn't run the shell; it drops a
   single byte into a lock-free ring and returns immediately. A scheduled shell
   task drains that ring on its own time. "Interactive," it turns out, is nothing
   more exotic than a task polling a queue that an interrupt keeps filling.

2. **"Done" as observable behavior.** A live prompt under QEMU. `help` lists five
   commands; `echo hello world` prints `hello world`; `clear` wipes the screen and
   redraws the prompt; `mem` shows *real* free-frame and heap numbers; `ps` lists
   the idle and shell tasks with their states and a tick count that rises as you
   watch; backspace erases; an unknown command reports itself cleanly. Crucially
   the shell runs as a **spawned task** — you can see it in its own `ps` output,
   not hiding inline in `kernel_main`. CI marker: `M10: shell online`.

3. **The smallest version that still teaches it.** Five built-ins, a 128-byte
   fixed line buffer, a lock-free single-producer/single-consumer ring of ASCII
   bytes, whitespace-split parsing, and a flat match on the verb. No history, no
   arrow keys, no pipes, no `exit`. And stage 1 — keyboard-to-ring plus a task
   that just echoes a prompt — is *itself* a complete, demoable artifact before a
   single command exists.

4. **Working vs. accidentally working.** Several traps, each with a guard:
   - `ps` has to show the shell as a **real task**. If it's secretly an inline
     loop in `kernel_main`, `ps` listing it proves nothing — the whole point is
     that it's scheduled.
   - The shell must survive being **preempted mid-command**. The line buffer is
     task-local (it lives on the shell's own stack), so a timer tick partway
     through typing can't corrupt it.
   - `ps` and `mem` must **never hold the scheduler or heap lock across a
     `print!`**. The timer's preemption could re-enter the scheduler and deadlock.
   - The idle task must `hlt`, not spin — otherwise the "idle" CPU is pinned at
     100%.

5. **What most likely stalls this for days.** A deadlock **hang**, not a
   triple-fault — the quiet kind. Two ways in: the IRQ taking a lock the shell
   already holds (removed by making the IRQ *enqueue-only*), or `ps`/`mem`
   printing while holding a lock. Mitigations, written down before coding: no
   interrupt handler touches the VGA lock, no lock is ever held across a `print!`,
   and if it does hang, attach GDB to *both* contexts (the interrupted task and
   the handler) to see who holds what.

6. **Drifting toward a non-goal?** Three temptations. Real block/wake for the
   shell task (deferred — `hlt`-poll is the honest minimal cut). Command history
   and readline-style line editing (out — that's a text-editor sub-project).
   `mem`/`ps` growing into a `/proc`-style filesystem (that's M11's job, not
   this one). Ran `/scope-guard` → **Hold**.

## Scope-guard

Verdict: **Hold**. Five built-ins are the *minimum* set that proves the input path
end to end: `echo` exercises argument parsing, `clear` exercises VGA control,
`mem` and `ps` exercise reading real live kernel state, and `help` exercises
discovery. Everything tempting is deferred *outward* into named work: block/wake
(the `hlt`-poll loop is the honest minimal version), history/readline (a
text-editor sub-project, not a shell milestone), and a `/proc`-style backing for
`mem`/`ps` (M11). The one thing that might *look* like scope creep — making the
shell a scheduled **task** instead of an inline loop — is the opposite: it
honestly *uses* M9, which is exactly why `ps` can show something true about
itself. And the lock-free ring is required complexity, not gold-plating: it's what
removes a real deadlock between the keyboard IRQ and the shell.

## Eng-plan (summary)

A lock-free ring feeding a shell task that reads pure functions:

- **`src/keyboard.rs`** gains a lock-free SPSC ring of ASCII bytes with
  `push`/`pop`. The keyboard IRQ is rewritten from "decode and echo" to
  "enqueue and return" — the single producer. This removes the latent hazard of
  the IRQ ever taking the VGA lock.
- **`src/shell.rs`** is the consumer: a shell task running a `hlt`-poll REPL. It
  pops bytes from the ring, edits a task-local 128-byte line buffer (echo +
  backspace), and on Enter parses and dispatches. The core logic is factored into
  **pure functions** — `Line::edit` returns an `Edit`, `parse` returns a
  `Command` — so it can be tested deterministically on serial without a keyboard.
- **`kernel_main`** now `spawn`s the shell and becomes the idle task; `task::init`
  and `task::spawn` were made `pub` so the shell can be spawned from outside the
  task module.
- **`mem`** reads `heap::stats()`; **`ps`** reads `task::snapshot()`, an
  IF-guarded read of live scheduler state (task ids, states, tick count).

Staging mirrors M9's risk isolation: get keyboard-to-ring plus a prompt-echoing
task working first (a complete artifact), then add command dispatch, then wire
`mem`/`ps` to real state. Full plan in
[docs/planning/milestone-10-eng-plan.md](../planning/milestone-10-eng-plan.md).

## What got built

Seven commits on `claude/ziran-os-plan-qgobid`:

- **M10 plan** — office-hours + scope-guard + eng-plan.
- **Step 1 — the input path.** A lock-free SPSC keyboard ring
  (`keyboard::push`/`pop`) and the IRQ rewritten from echo to enqueue — which
  removes the latent "IRQ takes the VGA lock" deadlock. `src/shell.rs` lands with
  the `hlt`-poll REPL, echo, and backspace; `task::init`/`spawn` made `pub`;
  `kernel_main` spawns the shell and becomes idle. Line editing is factored into a
  pure `Line::edit` for a deterministic self-test.
- **Step 2 — command dispatch.** A pure `parse() -> Command` and the
  `help`/`echo`/`clear` built-ins. The ring's FIFO round-trip is added to the
  self-test.
- **Step 3 — mem + ps against live state.** `task::snapshot()` (IF-guarded) and
  `heap::stats()`; `State` made `pub`; the final CI marker `M10: shell online`.
- **M10 review.** A 3-lens `/kernel-review`. It confirmed no *reachable* deadlock,
  then fixed two latent hazards: the boot self-test's ring-empty **assertion**
  (replaced with a drain, so a keystroke landing during boot can't panic) and made
  `snapshot` **allocation-free** while holding the scheduler lock (the
  allocate-under-an-IF-guarded-lock deadlock class). It documented that `spawn`
  carries the same latent pattern, and cleaned up nits (the ring is 255 usable of
  256 slots, a `parse` trailing-space comment, a `debug_assert` on ASCII).

## Verification status (honest)

Boot-tested live under headless QEMU. The serial log:

```
[ok] shell: editing, parsing, input ring, and ps/mem accessors verified (tasks=3, heap_free=1015568)
M10: shell online
[boot test] PASS -- reached long mode and 'M10: shell online'
```

What this actually proves — and, just as importantly, what it *doesn't*:

- The self-test drives the **pure logic** deterministically on serial: line
  editing (echo, backspace, submit), command parsing (including `echo`'s
  everything-after-the-verb tail and rejection of unknown verbs), and the ring's
  FIFO round-trip. If any of these were wrong, the assert would fail and the boot
  test would not reach the marker.
- It also drives the **`ps`/`mem` data accessors**: the idle task is present
  (`tasks=3`), the heap free figure is in a sane range (`heap_free=1015568`), and
  frames-free reads back. These pin the *numbers* the commands report, separately
  from how they're printed.
- **Interactive typing is a manual check.** The actual `echo`/`help`/`clear`
  output rendered on the VGA screen in response to real keystrokes is verified by
  hand under `make run` — CI can't type. But that's a thin shell around the logic:
  the editing and parsing are factored into pure functions and pinned by the
  self-test regardless, and the two genuinely manual pieces — the hardware
  keyboard IRQ and VGA rendering — were already proven in M4/M5.

The full 20-second run had no panics and no faults.

## Retro

**Plan vs. reality.** The design was accurate: the SPSC ring, the shell-as-task,
the `hlt`-poll REPL, and snapshot-then-print were all built as specified and
worked on the first try. The naive spot was one word away from being caught. The
plan carried M9's rule "don't hold the scheduler lock across a `print!`" — but
missed the adjacent trap: `snapshot` **allocating** a `Vec` while holding the
scheduler lock with `IF=0`. That's a *different* deadlock — a spin on the heap
lock against a task that was preempted mid-allocation and, with interrupts
disabled, can never be rescheduled to release it. Only the review caught it. The
plan also assumed the boot self-test could assert the ring starts empty; it can't,
because `sti` runs first and a stray keystroke could already be sitting in it.

**What broke, and for how long.** Nothing broke at runtime — zero debugging cycles
again. Every stage came up green on the headless boot plus the deterministic
self-tests. The two real issues (a boot panic on an early keystroke, and
allocate-under-lock) were found by the 3-lens review, not by a crash.

**The root cause / the fix.** Two, both structural, both found on paper:
   - *(a)* The self-test asserted `ring == empty` at boot, but the keyboard IRQ is
     already live at `sti`, which runs before the test — a stray keystroke would
     panic the assert. Fix: **drain, don't assert.**
   - *(b)* `snapshot` did a `.collect()` (a heap allocation) inside the scheduler
     lock with `IF=0`. A task preempted mid-allocation while holding the heap lock
     would never be rescheduled (interrupts are off), so the snapshot would spin
     forever. Fix: copy into a **fixed stack array** under the lock, and build the
     `Vec` *after* releasing it.

**The assumption worth re-examining (the lede).** *"The shell is interactive, so I
can't verify it automatically."* False — and worth dismantling, because it's a
reflex that quietly excuses untested code. An interactive program is a thin I/O
shell wrapped around pure logic. Split the two and the logic is as testable as
anything else in the kernel: `Line::edit` and `parse` don't know a keyboard
exists. What's left genuinely manual is only the hardware IRQ and the VGA
rendering — and both of those were already proven back in M4 and M5.

**What earned its keep.** Two things. First, **factoring the shell into pure
functions** (`Line::edit -> Edit`, `parse -> Command`) so a deterministic serial
self-test pins editing, parsing, the ring, and the accessors with no keyboard in
the loop. Second, the **adversarial 3-lens review**: with no crash to chase, it
was the *only* thing that surfaced the two latent hazards. A milestone with zero
runtime bugs is exactly the one where a crash-driven process finds nothing and a
review-driven one still earns its cost.

**One thing to do differently.** Add "no allocation inside a lock held with
interrupts disabled" to the eng-plan's edge-cases section as a **first-class
item**, right next to "no lock held across a `print!`." Both are the same family
of IF-guarded-lock trap that M9 first established — and here I applied only half
of it, catching the print case and missing the allocation case until review.

## Takeaway for the next person

An interactive program isn't a special, untestable thing — it's pure logic
wearing a thin I/O costume. The keyboard IRQ shrinks to "enqueue one byte and
return"; a scheduled task drains the ring on its own time; and the parts that make
it *feel* interactive (editing, parsing) are ordinary functions you can test
without ever pressing a key. Split the I/O from the logic and the "I can't verify
this automatically" excuse evaporates. The deadlock family to fear here is quiet:
a lock reached from an interrupt, or held across a `print!`, or — the one that got
past the plan — held across a heap allocation with interrupts disabled.

Next: **Milestone 11 — the filesystem.** The shell can read live kernel state and
run commands; next it gets something to read *from disk*. `mem` and `ps` hinted at
a `/proc`-style view of the machine; M11 makes it real storage, and the shell
grows the two commands every prompt eventually wants — `ls` and `cat`.
