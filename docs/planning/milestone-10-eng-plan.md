# Milestone 10 — simple shell — eng-plan

Locks the technical approach before building. Grounded in a two-way research
fan-out (the codebase integration points; the shell architecture). Companion
concept doc: `docs/concepts/shell.md` (written during the build).

The milestone teaches exactly one thing: **how keystrokes travel from a hardware
interrupt to a *program* without the interrupt doing the work** — the keyboard
IRQ becomes a pure *producer* (drop a byte in a queue, get out) and a scheduled
shell *task* is the *consumer* (drain the queue, echo, build a line, run
commands). "Interactive" turns out to be a task polling a queue an interrupt
fills.

## 0. Scope-guard (do this first)

A shell is where a teaching kernel is most tempted to grow a real userland.
Verdict **Hold**; the deferrals:

- **No block/wake.** The shell `hlt`s when its input queue is empty and the next
  interrupt (a keystroke, or the 100 Hz tick) wakes it to re-poll. True task
  parking (mark blocked, wake from the IRQ) is a named future refinement — it
  needs scheduler state this milestone deliberately doesn't add.
- **Five built-ins, nothing more.** `help`, `echo`, `clear`, `mem`, `ps`. No
  history, no arrow keys, no cursor movement, no pipes/redirection, no `exit`,
  no job control. A 128-byte fixed line buffer, whitespace-split parsing, a flat
  `match`.
- **`ps`/`mem` print a snapshot, they are not a `/proc` filesystem.** That's M11.
- **The shell is a scheduled task, not an inline loop.** This is the opposite of
  scope creep — it's the milestone honestly *using* M9 (so `ps` shows something
  true) instead of faking interactivity in `kernel_main`.

## 1. Approach

A new `src/shell.rs` plus a lock-free input ring and a handful of `pub`
exposures. No changes to the `unsafe`/asm switch core — the shell rides the
existing `switch_context`/`preempt` machinery unchanged.

**The input path (the crux) — a lock-free SPSC ring of raw ASCII bytes.** Exactly
one producer (the keyboard IRQ) and one consumer (the shell task), so a
single-producer/single-consumer ring of atomics is the textbook fit — and it
matches the house rule that IRQ-touched state uses atomics, never a lock
(`keyboard.rs` shift state, `pit.rs` ticks). Lives in `keyboard.rs` (or a small
`input.rs`):

```
const CAP: usize = 256;                          // power of two
static BUF:  [AtomicU8; CAP] = [AtomicU8::new(0); CAP];
static HEAD: AtomicUsize = AtomicUsize::new(0);  // consumer (shell) advances
static TAIL: AtomicUsize = AtomicUsize::new(0);  // producer (IRQ) advances

push(b):  tail = TAIL.load(Relaxed);
          next = (tail + 1) % CAP;
          if next == HEAD.load(Acquire) { return; }   // full -> drop newest (honest overflow)
          BUF[tail].store(b, Relaxed);
          TAIL.store(next, Release);                    // publish

pop() -> Option<u8>:
          head = HEAD.load(Relaxed);
          if head == TAIL.load(Acquire) { return None; } // empty
          b = BUF[head].load(Relaxed);
          HEAD.store((head + 1) % CAP, Release);
          Some(b)
```

The **keyboard IRQ arm** (`interrupts.rs`) changes from *echo* to *enqueue*:
```
KEYBOARD_VECTOR => {
    if let Some(c) = keyboard::handle_interrupt() { keyboard::push(c as u8); }
    pic::send_eoi(1);
}
```
Storing `u8` is sound: `keyboard::translate` only ever yields ASCII (`\n`,
`\u{8}`, space, printable). This refactor also **removes a latent deadlock**: the
IRQ no longer takes the VGA `WRITER` lock (see §4).

**The shell task.** `shell_main` is `extern "C" fn()` (matches `Task::new`),
calls `interrupts::enable()` first (tasks start with IF=0), prints a banner + the
first prompt, then runs the REPL forever (never exits, so `task_exit` never fires
for it):
```
loop {
    match keyboard::pop() {
        Some(b) => handle_byte(b),                 // echo / edit / dispatch
        None    => asm!("hlt"),                    // sleep until the next IRQ
    }
}
```
`hlt` (not `yield_now`, not a bare spin): when the shell is the only ready task
`yield_now` no-ops (`next == old`), so a yield/spin loop pegs the core; `hlt`
sleeps until the timer (≤10 ms) or a keystroke IRQ wakes it. Safe only because
the shell runs IF=1 and the lock-free ring means it holds no lock across the
`hlt`.

**Line editing:** a heapless `struct Line { buf: [u8; 128], len: usize }`.
Printable `0x20..=0x7e` → append + echo; Backspace `0x08` → `len -= 1` + echo
`\u{8}` (the VGA writer already blanks the cell); Enter `0x0a` → dispatch
`&buf[..len]` (valid UTF-8, all ASCII), reset, reprint prompt.

**Command dispatch:** `split_whitespace()`, first token = command, `match`:
`help` (list), `echo` (join the rest with spaces), `clear`
(`vga_buffer::clear_screen()` + reprint prompt), `mem` (frame + heap stats),
`ps` (task snapshot + ticks), unknown → `unknown command: X (try help)`.

**Exposures needed (all `pub`, all already IF-safe or trivially so):**
- `keyboard::push`/`pop` + the ring (new).
- `task::init`, `task::spawn` → `pub` (already interrupt-safe; currently private).
- `task::snapshot() -> Vec<(u64, State)>` (new; §3) with `State` made `pub`.
- `heap::stats() -> (usize used, usize free)` (new; walks the free list under the
  heap lock, returns after unlocking).
- `frame_allocator::total_frame_count()` (new, optional, for "used / total").

## 2. Preconditions

**On entry to the shell wiring** (`kernel_main` tail, after
`preemptive_self_test`):
- Long mode, heap up, paging on, PIC + PIT up, keyboard IRQ unmasked, and `sti`
  already executed (`lib.rs`) — interrupts are **live**.
- The preemptive self-test left a scheduler full of `Finished` tasks. Re-`init()`
  a fresh scheduler *before* spawning the shell, so `ps` shows a clean list (idle
  + shell) — re-init is the established pattern (both self-tests do it).

**The ring's memory-ordering contract (the one real correctness precondition):**
single-producer/single-consumer means each index has exactly one writer. The
producer publishes the byte with `TAIL.store(Release)` *after* writing `BUF`, and
the consumer reads `TAIL.load(Acquire)` *before* reading `BUF` — so a byte is
never observed before it's written. Symmetrically for `HEAD`. On a single core
this is really about forbidding *compiler* reordering (there's no second core to
race), but the Acquire/Release pair states the contract correctly regardless.
`BUF` cells use `Relaxed` (the index ordering fences them).

**The shell task's IF state:** starts IF=0 (reached via `task_trampoline`'s `ret`,
like every task), so it **must** `interrupts::enable()` before the REPL, or the
`hlt` wedges forever (IF=0 + `hlt` = permanent halt) and no keystroke ever
arrives. When preempted it resumes IF=1 via `iretq`; the lock-free consumer never
`cli`s, so no `hlt`-with-IF-cleared window exists.

**On exit:** the shell never returns; `kernel_main` falls into `hlt_loop()` as the
idle task (task 0), woken by each tick to let the scheduler preempt back to the
shell.

## 3. Data flow

```
key press
  └─ IRQ1 -> isr_stub_33 -> interrupt_dispatch (IF=0, interrupt gate)
       KEYBOARD_VECTOR => keyboard::handle_interrupt()  (read 0x60, decode)
                          keyboard::push(c as u8)        (ring producer, lock-free)
                          pic::send_eoi(1)
       iretq                                             (no VGA lock touched)

shell task (IF=1):
  loop:
    keyboard::pop()  --Some(b)-->  handle_byte(b):
                                     printable -> line.push + print!(c)      (echo on the TASK's stack)
                                     0x08      -> line.pop  + print!('\u{8}')
                                     0x0a      -> dispatch(&line) ; line.clear ; print!(prompt)
                     --None----->  hlt   (sleep until next IRQ re-runs the loop)

dispatch("ps"):   snap = task::snapshot();   // save_and_disable -> lock SCHED -> copy (id,state) -> drop -> restore
                  for (id,state) in snap { println!(...) }   // print AFTER the lock is released
                  println!("ticks: {}", pit::ticks());
dispatch("mem"):  free = frame_allocator::free_frame_count();   total = total_frame_count();
                  (used, freeb) = heap::stats();                 // lock heap -> sum -> unlock
                  println!(...)                                   // print AFTER unlock
```

The handoff boundaries: **IRQ → ring** (atomics, no lock), **ring → task** (the
poll), **task → SCHED/heap** (snapshot under guard, release, then print). No lock
is ever held across a `print!`.

## 4. Edge cases

- **Ring full.** `push` drops the newest byte when `next == HEAD`. Bounded and
  honest; a keystroke lost under a 256-byte backlog is a non-event for a human
  typist. (Never overwrite unread data — that would corrupt the consumer's view.)
- **Ring empty.** `pop` returns `None` → the shell `hlt`s. Correct idle.
- **Line buffer full (128 chars).** Drop further printable input silently (or
  ignore); do **not** overflow the array. No line-wrap cleverness.
- **Backspace on an empty line.** `len == 0` → do nothing (no echo, no underflow);
  don't let the cursor walk back over the prompt.
- **Empty line (bare Enter).** `split_whitespace().next() == None` → just reprint
  the prompt.
- **Preempted mid-command / mid-print.** The shell runs IF=1; the timer preempts
  it. The line buffer is task-local (on the shell's stack), so a switch preserves
  it (callee-saved / stack, per M9). Printing may interleave with... nothing —
  idle doesn't print (§ below), so a preempt mid-`print!` is benign.
- **`hlt` with IF=0.** Guarded by the mandatory `interrupts::enable()` at shell
  start and never `cli`-ing on the consumer side. This is the one that wedges
  silently — call it out.
- **`ps`/`mem` re-entrancy.** If the snapshot held `SCHED` across the `print!`,
  the 100 Hz timer would fire → `preempt` → `yield_now` → `SCHED.lock()` →
  deadlock. Snapshot-then-print is the fix; the accessor mirrors `spawn`'s
  IF-guard exactly.
- **Non-ASCII / multi-byte scancodes (arrow keys, F-keys).** `translate` already
  returns `None` for these, so they never reach the ring. No handling needed.
- **Re-init wiping the shell.** Spawn the shell *after* the last self-test's
  `init()`, or that `init()` would drop the shell task.

## 5. Verification plan (working vs. accidentally working)

The subtle failure here is concurrency (deadlock/hang), not whether `echo` echoes.

1. **The input path, end to end (stage 1).** Type at the prompt: characters echo,
   Backspace erases (VGA cell blanks), Enter starts a fresh prompt line. Proves
   IRQ→ring→task→VGA works. *Accidental-success guard:* stress it by typing
   continuously while the 100 Hz timer fires — a dropped/duplicated/corrupted
   char would mean the ring's ordering or the preempt-mid-edit handling is wrong.
2. **The shell is a real task, not inline.** `ps` must list the **shell task**
   (its own id, `Running`) alongside the idle task (id 0). If the shell isn't in
   the list, it's secretly running in `kernel_main` — the milestone's central
   claim (interactivity via the scheduler) would be a lie.
3. **No deadlock under `ps`/`mem`.** Run `ps` and `mem` repeatedly while the timer
   preempts; a hang would expose a lock held across a `print!`. (This is the
   office-hours Q5 failure mode — it *hangs*, it doesn't crash, so GDB is the tool.)
4. **Idle doesn't peg the core.** With the shell waiting at the prompt, the CPU is
   halted between ticks (not spinning). Observable via QEMU CPU usage, or by
   reasoning: the `hlt` path, not a spin.
5. **`mem`/`ps` show *live* state.** `mem` frame count should drop after the shell
   allocates (if it does); `ps` tick count should visibly rise between two runs —
   proving they read real kernel state, not constants.
6. **The removed deadlock (stage 1, silent win).** After the refactor, no
   interrupt handler takes the VGA/serial lock. Sanity: grep the IRQ arms — the
   keyboard arm enqueues, the timer arm ticks+preempts, neither prints.
7. CI marker `M10: shell online`; the Makefile `run-headless` grep updated from
   the M9 marker to this one. *Caveat:* `run-headless` can't type, so it asserts
   the marker (shell spawned + banner printed), not interactive input. Interactive
   typing is verified manually under `make run` (windowed) — state that honestly.

**GDB plan for the hang class (office-hours Q5):** a deadlock, unlike M9's
triple-fault risk, leaves the machine alive but stuck. `make debug` + inspect
*both* contexts: the shell task (spinning on a lock?) and the pending IRQ (spinning
on the same lock?). The rule that prevents it: no handler touches the writer lock,
no lock held across `print!`. `/investigate` 3-attempt rule applies.

## 6. Build order (each its own small commit, QEMU-tested)

De-risks by proving the input path before any command exists.

1. **Keyboard → ring + shell task echo.** The SPSC ring (`push`/`pop`); rewrite
   the keyboard IRQ arm to enqueue; `src/shell.rs` with `shell_main` doing
   enable → prompt → poll/`hlt` REPL with echo + backspace (no commands — Enter
   reprints the prompt). Make `task::init`/`spawn` `pub`; `kernel_main` re-inits,
   spawns the shell, `yield_now`, then idles. **Demo:** live prompt, echo,
   backspace, Enter. Proves the whole path (and silently retires the IRQ/VGA
   deadlock). Interim marker `M10: shell task online`.
2. **Command dispatch: help / echo / clear.** Parse + `match`; the three commands
   needing no new kernel accessors. **Demo:** `help`, `echo hello world`, `clear`,
   unknown-command report.
3. **mem / ps against real kernel state.** `task::snapshot()` (IF-guarded, `State`
   pub), `heap::stats()`, optional `frame_allocator::total_frame_count()`; `mem`
   and `ps` snapshot-then-print. Final marker `M10: shell online`; Makefile marker
   updated. Then `/kernel-review` → `/retro` → `/document-milestone` (concept doc
   `docs/concepts/shell.md`, blog post, `web/` step, STATUS/README).

## 7. Open unknowns (honest)

- **`const` array init of `[AtomicU8; 256]`.** `[AtomicU8::new(0); 256]` needs the
  inline-const `[const { AtomicU8::new(0) }; CAP]` form on the pinned toolchain (a
  non-`Copy` type in an array repeat). Will confirm at build; fall back to a
  `struct`-wrapped `UnsafeCell`/`[u8; N]` behind the SPSC discipline if the
  atomics-array form fights the compiler.
- **`heap::stats()` walking the free list.** Summing free bytes means traversing
  the intrusive `ListNode` chain under the heap `Mutex`. Confirm it allocates
  nothing (it must not — allocating while holding the heap lock would deadlock)
  and that the walk is O(free-regions), fine at this scale. If it's fiddly, the
  minimal honest `mem` reports `HEAP_SIZE` total + frame free/total and defers
  heap-used to a later refinement.
- **Whether `yield_now` after spawn is needed** vs. letting the next timer tick
  pick up the shell. Leaning explicit `yield_now` (instant, deterministic hand-off
  for the demo); will confirm both work.
- **Backspace visually walking over the prompt.** The VGA writer blanks the
  previous cell on `\u{8}`; need to ensure the shell's `len==0` guard stops
  backspace before it eats the `> ` prompt. Will eyeball under `make run`.
