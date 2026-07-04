# The first wall

*Milestone 13 — ring 3, a syscall gate, and the two bits that turn "the kernel is
the only program" into "the kernel runs code it doesn't trust." The first time in
this project that the **hardware**, not the kernel's own goodwill, holds code
back.*

> New to this? Read [docs/concepts/privilege.md](../concepts/privilege.md)
> alongside — it explains rings and CPL, the two bits that *are* the boundary (a
> segment's DPL and a page's U/S), the TSS and the triple-fault it prevents, the
> `iretq` trick for *entering* ring 3, and why the CPU ANDs the user/supervisor
> bit down the whole page walk. This post assumes those ideas; the concept doc
> builds them from nothing.

Every milestone up to now ran in **ring 0** — the CPU's most privileged mode,
where every instruction is legal and every byte of memory is reachable. The
shell, the file manager, the filesystem parser: all one program, all trusted,
because it was all mine. That is exactly what a real OS cannot do. Its whole job
is to run programs it *doesn't* trust and still guarantee they can't scribble on
the kernel or halt the machine.

Milestone 13 builds that guarantee for the first time. And it taught me one thing
worth the whole milestone:

> **A privilege boundary is enforced by the CPU, not by the kernel.**

You never write an `if` that asks "is this code allowed to touch that page?" You
set **two bits** — one in a segment descriptor, one in a page-table entry — and
from then on the silicon checks them on every single instruction, for free, with
zero kernel involvement. The kernel only *configures* the boundary. Getting those
two bits right, and standing up the plumbing so that crossing the boundary lands
on a valid stack instead of triple-faulting, is the entire milestone.

### Milestone 13 — userspace / ring 3 / syscalls — checklist

```
Think
- [x] /milestone-office-hours — six forcing questions answered in the eng-plan
- [x] /scope-guard — verdict: Hold, with an explicit deferral list

Plan
- [x] /eng-plan — approach, machine-state preconditions, edge cases, verification

Build
- [x] Working, demoable state: a program runs in ring 3, syscalls, and returns
- [x] `make` clean: assembles, compiles, links; every new unsafe/asm says why
- [x] No new compiler warnings

Debug
- [x] No multi-day bug hunt — the two showstoppers were designed around, not hit

Review
- [x] /kernel-review — no critical/high; 4 low-severity findings fixed

Security
- [x] /red-team — a real privilege boundary exists; 9/9 escapes blocked, one leak

Reflect
- [x] /retro — the honest post-mortem (below)
- [x] /document-milestone — STATUS.md ✅, README, this post
- [x] Teaching tool updated — a ring-3 tour step + the [m13] boot lines
- [x] Committed in small, self-tested steps
```

## Office hours: what "done" means

The forcing question that shaped everything was Q4 — *how will I know it's
actually working, versus accidentally working?* The trap here is a demo that
looks perfect but was secretly still in ring 0 the whole time (a mis-wired `iretq`
frame whose `CS` had privilege level 0). Then nothing is being enforced and the
whole thing is theater.

So "done" is not "it printed a `Z`." Done is:

1. A user program runs at **CPL 3** and makes one syscall (`int 0x80`), serviced
   with the saved `CS` showing `0x1b` — privilege level 3.
2. A second program **deliberately breaks the rules** — runs `cli`, reads a
   kernel page — and the CPU faults, caught with the saved `CS` still showing
   CPL 3.

Point 2 is the real gate. "It printed and didn't crash" proves nothing; "it tried
to cheat and the hardware said no, from CPL 3" proves the boundary exists.

## Scope-guard: the biggest creep magnet in the roadmap

"Userspace" pulls hard toward a real process model, an ELF loader, a POSIX-ish
syscall ABI, `fork`/`exec`, multiple processes — every one a PLAN §1 non-goal. The
verdict was **Hold**, with a tight cut: ship *exactly one* user execution — a
byte blob copied into a user-mapped page, entered via `iretq`, making one syscall
— plus the deliberate-violation test. Explicitly deferred: a real ABI, a process
model, an ELF loader, multiple processes, copy-from-user validation (the blob
passes its argument in a register — no user pointer to validate *yet*), a libc,
and scheduler integration of ring-3 tasks. Each is a clean future milestone;
folding them in here would have been the scope creep the plan exists to stop.

## Eng-plan: four cooperating pieces

The milestone is four bits of hardware that all have to be right at once, or the
machine silently reboots:

- **The GDT and a TSS.** The boot GDT had two entries (null + one ring-0 code
  segment). Ring 3 needs a user code and user data descriptor marked privilege
  level 3, and — the part that triple-faults you — a **Task State Segment** whose
  `RSP0` field tells the CPU which kernel stack to switch to the instant a ring-3
  program traps down to ring 0. No valid RSP0 → the trap can't get a stack →
  double fault → triple fault → silent reboot, no output. So the GDT gets rebuilt
  in Rust (`src/gdt.rs`), because the TSS descriptor needs a runtime address the
  assembler can't know.
- **The `USER` page bit.** Every mapping so far left the user/supervisor bit at 0
  (kernel-only) — which is *why* ring 3 can't read kernel memory; that was true
  for free. The subtlety that is the whole game: the CPU **ANDs U/S down the
  entire page walk**, so a user page needs the bit set on the leaf *and every
  table above it*. `map_user_page` (`src/paging.rs`) sets it on the leaf, on the
  tables it creates, and OR-s it into the shared upper tables — safe because the
  kernel's own leaves stay U/S=0, so the AND still denies ring 3.
- **The syscall gate.** `int 0x80`, re-armed as a privilege-level-3 gate so ring 3
  is even allowed to invoke it (a ring-0 gate would itself fault). Number in
  `rax`, argument in `rdi`, return in `rax` — deliberately tiny.
- **The `iretq` in and out.** The CPU has no "enter ring 3" instruction; it only
  ever *lowers* privilege by returning from an interrupt. So to enter ring 3 the
  first time, `boot/usermode.asm` fabricates the exact frame an interrupt would
  have pushed and `iretq`s into it. Coming back rides the same path in reverse:
  the `SYS_EXIT` handler **rewrites the saved interrupt frame** to point at a
  kernel continuation, and the ISR's own `iretq` lands back in ring 0.

## What got built

Five small commits, each self-tested and booted before the next:

1. `src/gdt.rs` — the Rust GDT + TSS. Self-test reads the task register back and
   asserts `tr = 0x28` — the TSS is *loaded*, not just built.
2. `src/paging.rs` — the `USER` flag + `map_user_page`; a self-test re-walks the
   tables and asserts U/S = 1 at all four levels plus the leaf.
3. `src/interrupts.rs` — the privilege-level-3 `int 0x80` gate + a `syscall`
   dispatch arm, proven from ring 0 first (it reports CPL 0, honestly).
4. `src/usermode.rs` + `boot/usermode.asm` — the excursion. A 21-byte blob
   (`mov eax, SYS_PRINT; mov edi, 'Z'; int 0x80; mov eax, SYS_EXIT; int 0x80`)
   dropped into a user page and run. Prints `M13: userspace online`.
5. The enforcement test — two blobs that break the rules on purpose.

## Verification status (honest)

The claim of a privilege milestone is "the CPU really was in ring 3," so the proof
has to come from the hardware, not just a serial `println`. Two sources:

- **Serial**: `SYS_PRINT 'Z' (CS=0x1b, CPL=3)`, then both violations
  `blocked ring-3 violation: #GP … CS=0x1b (CPL=3)` and `#PF … (U/S bit set)`.
- **QEMU `-d int`**, the emulator reporting each trap itself:

  ```
  v=80 e=0000 i=1 cpl=3 IP=001b:000000005000000a  ← the int 0x80, from CPL 3
  v=0d e=0000 i=0 cpl=3 IP=001b:0000000050000000  ← #GP on cli, CPL 3
  v=0e e=0005 i=0 cpl=3 IP=001b:0000000050000000 CR2=00000000000b8000  ← #PF
  ```

  `cpl=3`, `CS=0x1b`, the `#PF` error code `0x5` (present + user), CR2 pointing at
  the kernel page it tried to read. That log *is* the evidence.

## Red-team: attacking my own wall

A boundary you only tested from the inside is not a boundary you've tested. So
before calling it done, I attacked it — twelve ring-3 blobs, each trying to escape
([full write-up](../planning/milestone-13-red-team.md)).

**Nine escape attempts, all blocked, all from CPL 3.** `hlt`, reading and writing
`cr3`, `wrmsr`, and port I/O in both directions → **#GP** (privileged instruction
or the IOPL check). Writing the kernel's VGA buffer, reading the kernel image, and
*jumping into kernel code* → **#PF** (the U/S=0 leaf — and note the third one: the
instruction *fetch* walks the page tables too, so ring 3 can't even jump into the
kernel). An unknown syscall number came back as −1, no crash.

And one genuine finding: **`sgdt` and `sidt` do not fault from ring 3.** By a
decades-old x86 design wart, the instructions that *read* the GDT and IDT register
were never gated to ring 0, so a user program can learn the exact addresses of the
kernel's descriptor tables. It can't *write* them (`lgdt`/`lidt` are privileged),
so it's not an escape — but it is a real information leak, the kind used in the
real world to defeat kernel address randomization. Intel added a control bit
(`CR4.UMIP`) to close it; Ziran doesn't set it, because that would be *hardening*,
a non-goal. The value is knowing the gap is there.

## Retro: the uncomfortable honest part

Almost nothing broke, and that is the headline I have to be upfront about. There
was no two-evening page-fault hunt. The two things that *should* have eaten days —
the triple-fault on a bad RSP0, and the user page faulting on its first fetch
because an intermediate table lacked U/S — never happened, because the eng-plan
had already named them and the code was written to avoid them from the start.

This milestone was AI-assisted, and the shape of the work changed accordingly.
The value didn't come from *finding* a bug; it came from *front-loading* the
reasoning — into the eng-plan and the concept doc — so the bugs never compiled.
The "hours to find the bug" number, which is usually the point of these posts, is
close to zero for M13. What replaced it: the hard parts moved to the front, stated
as risks and designed around, and the honesty moved to the *review* and the
*red-team* — the two low-severity fixes review caught (a `map_user_page` that
could loosen shared page tables on an errored call; a self-test that would call a
spurious fault a clean round-trip), and the `sgdt`/`sidt` leak that only surfaced
because I attacked my own boundary instead of admiring it.

The one process assumption worth flagging: the temptation to call a privilege
boundary "done" because the happy path printed a `Z`. It wasn't done until it had
been attacked from the outside.

## Takeaway for the next person

If you're standing up ring 3 for the first time:

- **Build the TSS first, and prove it's loaded** (`str` → `0x28`) before any
  ring-3 code exists. The triple-fault-on-bad-RSP0 is the thing that eats days,
  and it's invisible — no output, just a reboot. Design it away; don't debug it.
- **Set U/S on every level of the walk, not just the leaf.** The CPU ANDs it all
  the way down. One missing bit on an intermediate table and your program faults
  on its very first instruction, for a reason that looks nothing like the cause.
- **The proof is the saved `CS`, not the absence of a crash.** Assert `CS & 3 ==
  3` on the syscall path. And then *attack the boundary from the outside* — the
  enforcement test and a red-team pass are what turn "it ran" into "it's real."
- **`-d int` is your primary evidence** for anything privilege-related: it reports
  the vector, the CPL, the CS selector, the error code, and CR2, straight from the
  emulator. For this milestone it isn't a debugging aid; it's the proof.

The wall is up. Next, the security track gets to try to climb it.
