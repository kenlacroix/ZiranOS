# Teaching it to fail gracefully

*Milestone 4 — the GDT/IDT and exception handling. Where the kernel stops
silently rebooting on a mistake and starts telling you what went wrong.*

> New to this? Read [docs/concepts/interrupts.md](../concepts/interrupts.md)
> first — it explains every term below from scratch.

This post follows the project's workflow (adapted from gstack): the milestone
was framed with forcing questions, planned, built, and reviewed. The framing and
plan below aren't after-the-fact tidying — they're what shaped the code.

## Office hours: sharpening the milestone first

Before writing a line, the six forcing questions:

1. **The one thing I'll understand after this:** how a CPU hands control to the
   kernel when something interrupts normal execution — the IDT and the
   interrupt calling convention.
2. **"Done" as observable behavior:** the kernel deliberately triggers a
   breakpoint (`int3`), prints that it caught it, and *keeps running*. And an
   unhandled fault prints a named report instead of rebooting.
3. **Smallest version that still teaches it:** an IDT wired for all 256 vectors,
   but only a handful of handlers with real logic (breakpoint, page fault, a
   generic fatal reporter). No TSS/IST yet.
4. **Working vs. accidentally working:** the `int3` self-test is the guard —
   if *any* link in the chain (table layout, stub, dispatch, `iretq`) is wrong,
   the machine faults or hangs instead of printing "survived the breakpoint."
5. **What could stall this for days:** a mismatch between the register order the
   assembly stub pushes and the Rust struct that reads it — every field would be
   silently wrong. Named it up front so I'd check it explicitly.
6. **Scope check:** in bounds. (See scope-guard below.)

## Scope-guard

Verdict: **Hold.** Interrupts are foundational, not creep — every later
milestone (keyboard, timer, preemption) depends on them. One deliberate
*reduction*: skip the TSS/IST double-fault stack for now. It's the "robust"
answer, but it adds a second fiddly descriptor format to a milestone whose lesson
is the IDT. Deferred, with the `ist` field already stubbed for when it comes.

## Eng-plan

**Approach.** A 256-entry IDT built in Rust (`src/interrupts.rs`). Per-vector
entry stubs generated in assembly (`boot/isr.asm`), because stable Rust can't
express the `iretq` entry/exit shape. Stubs normalise every fault into one
`InterruptContext` layout and call one Rust dispatcher.

**Machine-state preconditions.** Runs in 64-bit long mode with the boot GDT
still loaded (handlers use its code selector `0x08`). Interrupts stay *disabled*
the whole milestone — we never `sti` — so only synchronous exceptions can fire.
That's deliberate: no hardware IRQ can surprise us until the keyboard milestone.

**Edge cases named.** Error-code vs. no-error-code vectors (uniformised with a
dummy push); the split-across-three-fields handler address in an IDT entry;
16-byte stack alignment at the `call` into Rust; the page-fault address living in
CR2, not on the stack.

**Verification plan.** (1) It assembles/compiles/links with the Multiboot header
intact and no undefined symbols. (2) Under QEMU: the `int3` self-test prints and
execution continues; forcing a fault (e.g. a wild pointer write) prints a named
page-fault report with a CR2 address rather than rebooting. (2) runs in CI.

## What got built

- **`boot/isr.asm`** — a nasm macro emits all 256 stubs plus a shared
  `isr_common` trampoline and an `isr_stub_table` the Rust reads. The
  error-code-bearing vectors (8, 10–14, 17, 21, 29, 30) are handled distinctly
  from the rest.
- **`src/interrupts.rs`** — the `IdtEntry`/`Idt` types, `init()` that fills and
  `lidt`s the table, and `interrupt_dispatch`, which decodes the vector: `#BP`
  reports and resumes; `#PF` reports CR2 + a decoded error code and halts;
  everything else reports and halts legibly.
- **`kernel_main`** now calls `interrupts::init()` and fires the `int3`
  self-test.

## Verification status (honest)

Assembles, compiles, and links cleanly — valid Multiboot2 image, no undefined
symbols, no warnings. The interactive behavior (breakpoint recovery, page-fault
report) is exercised by CI's headless QEMU boot, not yet on real hardware. When
it *is* run and something inevitably misbehaves, the debugging story goes here —
that's the honest, interesting part, and it's not invented in advance.

## What the next person should take away

The subtle, transferable idea isn't "call `lidt`." It's that an interrupt is a
**contract written in two languages**: assembly agrees to leave the machine
state in a precise shape on the stack, and Rust agrees to read exactly that
shape. The whole milestone lives or dies on those two descriptions matching.
Everything else is bit-packing you can look up.
