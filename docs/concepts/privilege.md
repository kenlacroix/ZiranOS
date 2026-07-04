# Privilege rings, from first principles

*Companion to Milestone 13. Read this before `src/gdt.rs`, `src/usermode.rs`,
and `boot/usermode.asm` — it explains what those files are *for* and why "run
some code in userspace" hides four cooperating pieces of hardware machinery.
Builds on two earlier docs: [interrupts.md](interrupts.md) gave you the GDT, the
IDT, and the `iretq` that returns from an interrupt; [virtual-memory.md](virtual-memory.md)
gave you page tables and the flag bits in a page-table entry — including one bit,
U/S, we ignored until now. The one thing you need coming in: every milestone
before this ran with **full power over the machine**, and nothing stopped it.*

---

## The problem this solves

Up to Milestone 12 the kernel *was* the only program. The shell, the file
manager, the filesystem parser — all of it ran in **ring 0**, the CPU's most
privileged mode, where every instruction is allowed and every byte of memory is
reachable. There was no *inside* and *outside*, no code the kernel had to be
suspicious of, because it was all the kernel's own code.

That is exactly what a real operating system cannot do. The whole point of an OS
is to run programs it *doesn't* trust — a text editor, a game, something you
downloaded — and still guarantee that a bug or a malicious trick in one of them
can't scribble over the kernel, read another program's secrets, or halt the
machine. The program has to run **unprivileged**, and the kernel has to be able
to hand it the CPU and get it back safely.

Here's the idea that makes Milestone 13 worth doing, and it is not obvious:

> **A privilege boundary is enforced by the CPU, not by the kernel.**

You never write an `if` that asks "is this code allowed to touch that page?" You
set **two bits** — one in a segment descriptor, one in a page-table entry — and
from that moment the silicon checks them on *every single instruction*, for free,
with zero kernel involvement. The kernel's job is only to *configure* the
boundary correctly. Getting those two bits right, and standing up the plumbing so
that crossing the boundary lands on a valid stack instead of triple-faulting, is
the entire milestone.

## Rings: the CPU's four privilege levels

x86 has four privilege levels, **ring 0** (most privileged) through **ring 3**
(least). In practice everyone uses only two: ring 0 for the kernel, ring 3 for
user programs. (Rings 1 and 2 were meant for device drivers; almost no OS bothers.)

The CPU always knows which ring it is currently running in. That number is called
the **CPL** — Current Privilege Level — and it is not stored in some special
register you set directly. It **is the low two bits of the `CS` (code segment)
register**. If `CS = 0x08`, the low two bits are `0b00`, so CPL 0. If `CS = 0x1b`,
the low two bits are `0b11`, so CPL 3. Change what's in `CS` (only in the
controlled ways the CPU allows) and you change what ring you're in.

This is why the *proof* that Milestone 13 works is a single check: when a syscall
or a fault arrives, we look at the saved `CS`. If `CS & 3 == 3`, the CPU genuinely
was in ring 3. Nothing else — not "it printed and didn't crash" — settles it.

What does ring 3 actually forbid? Two categories:

1. **Privileged instructions.** `cli`/`sti` (disable/enable interrupts),
   `hlt` (halt the CPU), `mov` to/from control registers (`cr0`, `cr3`, …), `lgdt`,
   `ltr`, and friends. Run any of them at CPL 3 and the CPU raises a
   **#GP (General Protection fault)** instead of executing them.
2. **Memory the kernel marked off-limits.** This is the page-table U/S bit, below.

## The first bit: descriptor DPL (the segment side)

Recall from [interrupts.md](interrupts.md) that the **GDT** (Global Descriptor
Table) holds *segment descriptors*, and `CS`/`SS` are *selectors* — byte offsets
into that table. In long mode segments barely do anything anymore (no base, no
limit), but each descriptor still carries a **DPL** — Descriptor Privilege Level,
a 2-bit field. The DPL is the ring a descriptor belongs to.

The boot GDT (`boot/boot.asm`) had exactly two entries: the null descriptor and
one ring-0 code segment (DPL 0). To run ring-3 code we need descriptors the CPU
will *let* unprivileged code run under. So `src/gdt.rs` rebuilds the GDT with
five real entries:

| Selector | Descriptor | DPL |
|----------|------------|-----|
| `0x08` | kernel code | 0 |
| `0x10` | kernel data | 0 |
| `0x18` | **user code** | **3** |
| `0x20` | **user data** | **3** |
| `0x28` | the TSS (below) | 0 |

A selector's low two bits are the **RPL** (Requested Privilege Level), and for a
ring-3 segment we set them to match: the user code selector is used as
`0x18 | 3 = 0x1b`, the user data as `0x20 | 3 = 0x23`. Those are the exact values
you'll see in the saved `CS`/`SS` when a ring-3 program traps into the kernel.

Why rebuild the whole GDT in Rust instead of patching the assembly table? Because
of the fifth entry — the TSS — whose descriptor has to contain a *runtime*
address the assembler can't know. Once we own the GDT we own it end to end.

## The TSS and RSP0: where the stack comes from

Here is the part that triple-faults you if you get it wrong, and it is not
intuitive. When ring-3 code traps into the kernel — via a syscall or because it
faulted — the CPU switches from CPL 3 to CPL 0. But a ring-3 program runs on its
*own* stack (`RSP` points into user memory). The kernel cannot run on that stack:
it's user-controlled, possibly malicious, possibly unmapped. So the CPU needs a
**known-good kernel stack** to switch to, automatically, as part of taking the
trap — before a single kernel instruction runs.

Where does it get that stack pointer? From the **TSS** — the Task State Segment.
In 64-bit mode the TSS no longer holds task state (hardware task switching is
gone); it survives almost entirely for one field, **RSP0**, at byte offset 4: the
stack pointer the CPU loads when an interrupt or trap raises the privilege level
to ring 0. `src/gdt.rs` builds a TSS, points RSP0 at a dedicated 16 KiB kernel
stack, and loads it with the `ltr` instruction.

If there is no TSS loaded, or RSP0 is garbage, the sequence is brutal and silent:
the ring 3 → ring 0 trap can't get a stack → the CPU raises a **#DF (double
fault)** → the #DF handler also can't get a stack → **triple fault** → the machine
resets, with no output at all. This is why "stand up the TSS" is step 1 of the
milestone and gets its own self-test (`gdt::self_test` asserts the task register
reads back `0x28`). See the Milestone 13 eng-plan, question 5: *what could stall
this for days.*

## Getting *into* ring 3: the `iretq` trick

Now the strange asymmetry. The CPU has no "enter ring 3" instruction. It only
ever *lowers* privilege as a side effect of one thing: **returning from an
interrupt**. `iretq` pops a saved frame — `RIP`, `CS`, `RFLAGS`, `RSP`, `SS` — and
if the `CS` it pops has a lower privilege (CPL 3), the CPU drops to ring 3 as it
returns.

So to *enter* ring 3 for the very first time, we **fabricate the frame an
interrupt would have left** and `iretq` into it. That's all `boot/usermode.asm`'s
`usermode_enter` does:

```text
    push 0x23          ; SS     = user data selector | RPL 3
    push <user stack>  ; RSP    = top of the user stack
    push 0x2           ; RFLAGS = reserved bit 1; IF = 0
    push 0x1b          ; CS     = user code selector | RPL 3
    push <entry>       ; RIP    = the program's first instruction
    iretq              ; -> the CPU drops to CPL 3 and jumps to <entry>
```

The user "program" in Milestone 13 is a 21-byte, position-independent blob — a
handful of `mov`s and `int 0x80`s, no absolute addresses — copied into a page and
run. No ELF, no loader, no libc; those are deliberate non-goals for this cut (see
the eng-plan's scope-guard). The point isn't to run *interesting* user code; it's
to prove the boundary is real.

## The second bit: the page U/S flag (the memory side)

The segment DPL controls what *code* may run. The page-table U/S bit controls
what *memory* it may touch. Recall the page-table entry format from
[virtual-memory.md](virtual-memory.md): bit 2 is **U/S** (User/Supervisor).
`U/S = 0` means ring-0-only; `U/S = 1` means reachable from ring 3.

Every mapping the kernel made before Milestone 13 — the identity map, the heap —
left U/S = 0. *That is why ring 3 can't touch kernel memory: it was already true,
for free, the moment we entered ring 3.* We don't add protection; the protection
was the default, and userspace is the thing that has to be *explicitly* let in.

There is one genuinely subtle rule, and it is the whole game. **The CPU ANDs the
U/S bit down the entire page walk.** For ring 3 to read a page, U/S must be 1 on
the leaf **and on every table above it** — the PML4 entry, the PDPT entry, the PD
entry, *and* the final PT entry. Miss it on any single level and ring 3 faults,
even if the leaf itself says U/S = 1.

So `paging::map_user_page` (in `src/paging.rs`) sets U/S on the leaf, on every
intermediate table it creates, *and* OR-s it into any intermediate that already
exists on the path — including `PML4[0]`, which is shared with the kernel's own
identity map. That sounds alarming; it is not. Setting U/S = 1 on an *upper* entry
only **permits** a user access to keep walking downward. The kernel's own leaves
still say U/S = 0, and the AND makes the result 0 — kernel-only. Loosening the
parent grants nothing; the leaf is the real gate. (This is precisely what keeps a
future M15 secret page safe even though it lives under a user-permitted PML4
entry.)

## The syscall gate: how ring 3 asks for something

Ring 3 can't touch hardware or kernel memory — so how does a user program print a
character, or do anything useful? It **asks the kernel**, through the one door the
kernel controls: a software interrupt, the classic `int 0x80`.

From [interrupts.md](interrupts.md), an IDT entry (a "gate") has a DPL too. Every
gate the kernel installed was DPL 0 — and the CPU checks `CPL <= gate.DPL` on a
software `int`, so an `int 0x80` from ring 3 through a DPL-0 gate would itself
raise #GP. So `src/interrupts.rs` re-arms vector `0x80` as a **DPL-3 gate**
(`type_attr = 0xEE`): now ring 3 is allowed to invoke it. Everything else about
the path reuses the Milestone 4 machinery — the same stub saves the full register
context, and the CPU's automatic RSP0 stack switch (from the TSS) means the
handler runs on a kernel stack, not the user's.

The syscall convention is deliberately tiny: the number goes in `rax`, arguments
in `rdi`/`rsi`/`rdx`, the return value comes back in `rax`. Milestone 13 defines
two: `SYS_PRINT` (print the character in `rdi`) and `SYS_EXIT`. Passing the
character *by value in a register* is intentional — the moment a syscall takes a
*pointer* into user memory, the kernel must validate it (is it mapped? in range?
does it alias kernel memory?), and that "confused deputy" surface is the heart of
Milestone 15, kept out of this cut on purpose.

## Getting *back*: rewriting the frame

`SYS_PRINT` is easy: service it, write the return value into the saved `rax`, and
the interrupt path's normal `iretq` returns to ring 3 where it left off.

`SYS_EXIT` is the interesting one. We do *not* want to return to the blob — we
want to be back in the kernel, in ring 0, continuing the boot. There's a neat
trick that avoids any second mechanism: the handler **rewrites the saved
interrupt frame**. `usermode::resume_kernel` overwrites the saved `RIP`/`CS`/`SS`/
`RSP`/`RFLAGS` to point at a kernel continuation (`usermode_resume`) on the kernel
stack we stashed on the way in — and then the *same* `iretq` the ISR always runs
returns us to ring 0 instead of ring 3. (This works because 64-bit `iretq`
*always* pops `SS:RSP`, even when the privilege level doesn't change — unlike the
32-bit `iret` you may remember.)

## The enforcement, made visible

None of the above proves anything unless you watch the boundary *stop* something.
So Milestone 13 also runs two blobs that deliberately misbehave:

- **`cli` from ring 3** → the CPU raises **#GP**. Unprivileged code may not disable
  interrupts.
- **`mov rax, [0xb8000]` from ring 3** (a read of the kernel's VGA buffer) → **#PF**,
  with the error code's U/S bit set, telling you a *user-mode* access was denied.

The kernel's fault handler catches each one, checks that the saved `CS` shows
CPL 3, prints the violation, and unwinds back to ring 0 (the same frame-rewrite
trick). The serial log and QEMU's `-d int` both show the faults taken at `cpl=3`
under `CS=0x1b`. *That* is the milestone: not "it printed a `Z`," but "it tried to
cheat, and the hardware said no, from ring 3."

## Verification honesty: working vs. accidentally working

The failure mode to fear here isn't a crash — it's a demo that *looks* like it
worked but was secretly still in ring 0 the whole time (a mis-wired `iretq` frame
whose `CS` had RPL 0, say). Then nothing is being enforced and the whole thing is
theater. Three guards keep us honest:

- `gdt::self_test` reads the task register back and asserts it is `0x28` — the TSS
  is genuinely loaded, not just built.
- Every syscall and every caught violation checks `CS & 3 == 3`. `SYS_EXIT`'s
  handler *asserts* it, so a frame that left us in ring 0 panics loudly instead of
  passing silently.
- `usermode::self_test` confirms the excursion ended via `SYS_EXIT`, not by the
  blob spuriously faulting — so "returned to ring 0" can't be true unless the blob
  actually ran in ring 3.

## Where this goes next: the confused deputy (Milestone 15)

Milestone 13 builds the jail; **Milestone 15** breaks into it — and finds the one
door the CPU can't guard. It plants a `FLAG` on a kernel-only (U/S = 0) page and
attacks it two ways. The first is a *direct* ring-3 read of the secret's address:
the CPU faults it (#PF, U/S), exactly as the `0xb8000` demo above — the hardware
half of the boundary, now aimed at a real secret. That one holds for free.

The second is the lesson. M15 adds the kernel's first *pointer-carrying* syscall,
`SYS_WRITE(ptr, len)`, which copies bytes from a user buffer to the console. Now a
ring-3 program can ask the kernel to read `SECRET_VA` **on its behalf** — and the
kernel runs in ring 0, so when *it* dereferences that pointer, the CPU sees a
supervisor access and allows it. The ring/page boundary is completely silent. This
is the **confused deputy**: a privileged agent tricked into misusing its authority
for a caller who lacks it. The `#PF` that stops ring 3 from reading the page
directly does nothing when ring 0 reads it voluntarily.

The only defense is *software*: before the deputy acts, it must prove the caller
could have done the thing itself. That is `copy_from_user` — in Ziran,
`paging::user_range_ok(ptr, len)`, which walks the page tables (never the memory)
and requires `PRESENT | USER` at every level of the range, ANDing the U/S bit down
the walk exactly as the MMU would. M15 ships both the validated `SYS_WRITE` (which
rejects `SECRET_VA` with `-1`, reading nothing) and a deliberately-broken
`SYS_WRITE_UNCHECKED` (which skips the walk and leaks the 32 flag bytes) — the same
attack, one line of validation apart. **The lesson in one sentence: the CPU stops
*unauthorized access*, but only software stops *authorized misuse*.**

One subtlety the M15 red-team surfaced, worth keeping: validating *permission* is
not the same as validating *addressability*. The first version of `user_range_ok`
walked the tables (only bits 0–47) but never checked that the pointer was
**canonical** — so a pointer with high bits set, whose low 48 bits named a real
user page, passed the check and then `#GP`'d the *kernel* on the actual
dereference (a ring-3-triggerable crash, not a leak). A copy-from-user check must
cover every fault class the eventual deref can raise — #PF *and* #GP — not just the
one that motivated it. (The `sgdt`/`sidt` info-leak from M13 is still open; closing
it is `CR4.UMIP`, i.e. hardening — a deliberate non-goal.)

Other threads left open on purpose, each a clean future milestone: folding ring-3
tasks into the preemptive scheduler (which needs a per-task RSP0 and a way to
save/restore a ring-3 context across a timer tick); `syscall`/`sysret` instead of
`int 0x80` (faster, but it does *not* switch stacks for you — a footgun); an ELF
loader so the user program is a real binary rather than a hand-assembled blob; and
guard pages on the kernel stacks so a runaway fault reports instead of silently
eating `.bss`.
