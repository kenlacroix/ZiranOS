# Milestone 13 — red-team: attacking the ring-3 privilege boundary

Authorized self-testing of the author's own kernel in QEMU (PLAN §8). The goal is
**not** to harden — it is to learn what the boundary M13 just built actually
enforces, by trying to break it and recording what the hardware does. Every probe
names an invariant, tries to violate it, and reports held-or-leaked plus the
*mechanism*. "The boundary held, and here's why" is the common — and most
instructive — result.

Method: each attack is a tiny ring-3 blob run through the same `iretq` launch as
the M13 excursion, ending in `SYS_EXIT` so a *non*-faulting attack reports "no
fault" rather than hanging. The harness (temporary, reverted after capture)
printed `HELD` (the CPU raised a fault, caught at CPL 3) or `NO FAULT` (the
instruction executed in ring 3). Twelve probes, run 2026-07-04.

---

## Boundary 1 — the privilege boundary (ring 3 → ring 0)

**Invariant.** Ring-3 code cannot execute a privileged instruction, and cannot
read, write, or fetch from a page mapped supervisor-only (U/S = 0). The CPU
enforces this on every instruction from the two bits M13 configured (descriptor
DPL, page U/S); the kernel writes no check.

| # | Attack (from ring 3) | Result | Mechanism |
|---|----------------------|--------|-----------|
| 1 | `hlt` | **HELD** — #GP | privileged instruction, CPL 3 > 0 |
| 2 | `mov rax, cr3` (read control reg) | **HELD** — #GP | control-register access is privileged |
| 3 | `mov cr3, rax` (write control reg) | **HELD** — #GP | same — the CR3 that *is* the address space is untouchable |
| 4 | `wrmsr` (write a model-specific reg) | **HELD** — #GP | privileged |
| 5 | `in al, 0x60` (read the keyboard port) | **HELD** — #GP | CPL(3) > IOPL(0) and no I/O bitmap in the TSS |
| 6 | `out 0x60, al` (write a port) | **HELD** — #GP | same IOPL check |
| 7 | `mov [0xb8000], rax` (write kernel VGA) | **HELD** — #PF | leaf U/S = 0; error code shows write + user |
| 8 | `mov rax, [0x100000]` (read kernel image) | **HELD** — #PF | leaf U/S = 0 |
| 9 | `jmp 0x100000` (execute kernel code) | **HELD** — #PF | instruction *fetch* from a U/S = 0 page faults too |

**Every privilege-escape attempt was stopped, from CPL 3.** Two distinct
mechanisms did the work, exactly the two bits: a **#GP** for anything that tried
to *do* something privileged (instructions 1–6), and a **#PF** for anything that
tried to *touch kernel memory* (7–9) — read, write, or execute, all denied by the
same U/S = 0 leaf. Instruction 9 is the neat one: there is no "execute" bit to
set here, yet the fetch itself walks the page tables and faults, so ring 3 can't
even jump into kernel code. Port I/O (5–6) is worth noting: it's blocked not by a
page bit but by the IOPL check plus the TSS's empty I/O-permission bitmap
(`iomap_base` = TSS size), so ring 3 has no port access at all.

## Boundary 2 — the syscall interface (`int 0x80`)

**Invariant.** The syscall gate is the only way through, and it must not be
tricked into doing something on the caller's behalf that the caller couldn't do
itself (the "confused deputy").

| # | Attack | Result | Mechanism |
|---|--------|--------|-----------|
| 12 | `int 0x80` with `rax = 0xDEAD` (unknown number) | held (graceful) | the dispatch `_` arm returns −1; no crash, no leak |

The M13 syscall ABI passes its one argument (the char for `SYS_PRINT`) **by value
in a register**. There is *no user pointer for the kernel to dereference*, so
there is no confused-deputy surface to attack yet — which is exactly why this cut
is safe and exactly what M15 will change. Nothing here leaked.

## Findings — where it leaked

| # | Attack | Result | What leaked |
|---|--------|--------|-------------|
| 10 | `sgdt [rsp-16]` (store GDT register) | **NO FAULT** | the GDTR — base address + limit of the kernel's GDT — written into user memory |
| 11 | `sidt [rsp-16]` (store IDT register) | **NO FAULT** | the IDTR — base address + limit of the kernel's IDT |

**Finding (info disclosure, not privilege escape).** `sgdt`, `sidt`, `sldt`,
`str`, and `smsw` are, by a decades-old x86 design wart, **not privileged** — they
read a system register but the architecture never gated them to ring 0. So a
ring-3 program can learn the exact virtual addresses of the kernel's descriptor
tables. It cannot *write* them (`lgdt`/`lidt`/`ltr` are privileged — and untested
here only because they'd need a crafted operand; they are ring-0-only), so this is
not an escape. But it is a genuine **information leak**: in a real OS with kernel
address-space randomization, `sgdt`/`sidt` are a classic way to defeat it and
locate kernel structures for a follow-on attack.

**Mechanism / the fix that exists.** Intel added **UMIP** (User-Mode Instruction
Prevention, `CR4.UMIP`, bit 11, Skylake+) precisely to close this: with UMIP set,
`sgdt`/`sidt`/`sldt`/`str`/`smsw` from CPL > 0 raise #GP. Ziran doesn't set it.
Enabling it is a one-line `CR4` write — but that is *hardening*, a PLAN §1
non-goal. The value here is understanding the gap, not closing it. (If a later
milestone wants to *feel* the fix, flipping `CR4.UMIP` and re-running probes 10–11
turns the two `NO FAULT`s into `#GP`s — a decision you can watch land, like the
M16 extent check.)

---

## What this means for M15 (the flag capture)

The privilege boundary is **solid on every axis M15 will lean on**: ring 3 cannot
read, write, or execute a U/S = 0 page, and cannot run a privileged instruction.
So M15's plan holds — plant a secret in a kernel-only page, have a ring-3 program
try in earnest to read it, and the #PF (probe 8) is what stops it. The interesting
M15 work is therefore **not** on the page side (that boundary is proven) but on
the two seams this exercise found soft:

1. **The syscall argument path.** Register-only today (probe 12 had nothing to
   abuse). The moment `SYS_PRINT` — or a new `SYS_WRITE` — takes a *user pointer +
   length*, the confused-deputy surface opens: pass a kernel address as the
   buffer, an unmapped pointer, a huge length, a pointer that aliases the secret
   page. That is where copy-from-user validation must live, and where M15's
   fuzzing belongs.
2. **The `sgdt`/`sidt` leak.** Not a flag capture on its own, but the honest
   footnote: the boundary that blocks *access* to kernel memory does not blot out
   all *knowledge* of it. Worth a paragraph in the write-up, and the UMIP toggle
   is there if a future milestone wants to demonstrate the mitigation.

**Bottom line:** 9 of 9 privilege-escape attempts blocked (#GP or #PF, all at
CPL 3); the syscall handled a garbage number gracefully; the only leaks are the
non-privileged `sgdt`/`sidt` reads, an information disclosure that UMIP would
close. The wall M13 built holds — and now we know exactly where its one gap is.
