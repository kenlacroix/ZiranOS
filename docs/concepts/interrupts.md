# Interrupts and exceptions, from first principles

*Companion to Milestone 4. If you've never done kernel work, read this before
`src/interrupts.rs` and `boot/isr.asm` — it explains every term those files
assume. No prior OS knowledge needed; just the idea that a CPU executes one
instruction after another.*

---

## The problem this solves

A CPU running our kernel is executing instructions in a straight line. But the
outside world isn't a straight line. A key gets pressed. A timer ticks. Or the
code does something impossible — divides by zero, touches memory that isn't
there. In every one of these cases the CPU needs to **stop what it's doing, go
handle the event, and (sometimes) come back** as if nothing happened.

That mechanism is the **interrupt**. It's the single most important idea in how
an OS stays in control of a machine. Two flavors:

- **Interrupts** (hardware): something external wants attention — keyboard,
  timer, disk. Asynchronous; unrelated to the instruction currently running.
- **Exceptions** (faults): the current instruction itself went wrong — divide by
  zero, a bad memory access, an illegal opcode. Synchronous; caused by the code.

The CPU treats both almost identically, and so do we. Each is identified by a
number 0–255 called a **vector**. Vector 0 is "divide error." Vector 14 is "page
fault." Vector 3 is "breakpoint." The architecture fixes the meaning of vectors
0–31; the rest are ours to assign later (the keyboard will become one in
Milestone 5).

## What "handle it" means, mechanically

When vector N fires, the CPU asks a simple question: *where is the code that
handles N?* It answers by looking in a table you gave it — the **Interrupt
Descriptor Table (IDT)**. The IDT is just an array of 256 entries; entry N holds
(among other things) the address of the function to jump to for vector N.

You build that table in memory, then run one instruction — `lidt` ("load IDT") —
that tells the CPU where the table is. From then on, any fault is a table
lookup and a jump, instead of a catastrophe.

### Why a bad kernel "just reboots"

If vector 14 fires and the IDT has no valid entry for it, the CPU faults *again*
trying to handle the fault — a **double fault** (vector 8). If *that* also has no
handler, the CPU gives up and does a **triple fault**, which on real hardware
and in QEMU means: reset the machine. No message. That silent reboot is the
single most disorienting thing in early OS work, and installing an IDT is
exactly what replaces it with a printed explanation. That's why this milestone
is called "teaching it to fail gracefully."

## Two tables, easy to confuse: GDT vs IDT

- The **GDT (Global Descriptor Table)** describes *segments* — broad regions of
  memory and the privilege level (ring 0 = kernel, ring 3 = user) code runs at.
  In 64-bit long mode segmentation is mostly vestigial, but you still need a
  minimal GDT to say "here is a 64-bit, ring-0 code segment." We built one back
  in `boot/boot.asm` to get into long mode, and each IDT entry points back at
  that code segment (selector `0x08`) to say "run the handler at ring 0."
- The **IDT (Interrupt Descriptor Table)** is the subject of this milestone:
  vector → handler.

One-liner: **the GDT is about *who* code runs as; the IDT is about *what to do
when something interrupts it*.**

## The awkward part: the interrupt calling convention

Here's where it gets physical. When the CPU takes an interrupt, it doesn't
politely call a function. It **pushes some registers onto the stack itself** and
jumps to your handler. Specifically it pushes, in order:

```
SS      (stack segment)
RSP     (the stack pointer the interrupted code was using)
RFLAGS  (the CPU flags)
CS      (code segment)
RIP     (the instruction to resume at)
```

and, for *some* exceptions only, an **error code** below that (extra detail
about what went wrong — e.g. which selector, or the nature of a page fault).

Your handler must end with a special instruction, **`iretq`** ("interrupt
return"), which pops that frame back and resumes. A normal `ret` would be wrong —
it would only pop RIP and leave the stack corrupted.

Two consequences shape our code:

1. **Stable Rust can't write a raw interrupt handler.** The entry/exit shape
   (CPU-pushed frame, `iretq`) isn't something a normal `fn` produces. So we
   write a tiny **assembly stub** per vector (`boot/isr.asm`) that bridges into
   ordinary Rust.
2. **Some vectors push an error code and some don't.** That would give handlers
   two different stack layouts. We paper over it: stubs for the no-error-code
   vectors push a fake `0`, so *every* handler sees one uniform layout.

### What our stub does, in words

For each of the 256 vectors, `boot/isr.asm` generates a stub that:

1. pushes a dummy error code (only if the CPU didn't push a real one),
2. pushes the vector number (so the handler knows which fault fired),
3. jumps to one shared routine, `isr_common`, which
4. pushes every general-purpose register (so the handler can see, and we can
   restore, the exact interrupted state),
5. sets `RDI` to the stack pointer and `call`s the Rust function
   `interrupt_dispatch` — because the C calling convention passes the first
   argument in `RDI`, the handler receives a pointer to everything just saved,
6. on return, restores the registers, drops the vector + error code, and
   `iretq`s.

That saved pile of registers, read from Rust, is the `InterruptContext` struct
in `src/interrupts.rs`. **Its field order must exactly match the push order** —
get one field wrong and every value is misread. That coupling between two
languages is the subtle, worth-understanding core of this milestone.

## Anatomy of an IDT entry (and why it's weird)

Each IDT entry is 16 bytes, and the 64-bit handler address is split across
**three separate fields** with other data wedged between them:

```
offset_low   (bits  0..16 of the address)
selector     (which GDT code segment to run under -> 0x08)
ist          (special alternate stack index; 0 = use the current stack)
type_attr    (0x8E = present + ring 0 + "64-bit interrupt gate")
offset_mid   (bits 16..32 of the address)
offset_high  (bits 32..64 of the address)
reserved
```

Why the split? Backward compatibility. The 16- and 32-bit designs put other
fields where the upper address bits now live, and x86 never breaks an old
layout. `set_handler` in `src/interrupts.rs` reassembles the address by shifting.

`type_attr = 0x8E` picks an **interrupt gate**, which automatically disables
further interrupts while the handler runs — so a handler isn't itself
interrupted before it's ready. (A "trap gate," `0x8F`, would leave them on.)

## The one we can recover from: the breakpoint

Most faults in this milestone we can only *report*, then halt — recovering from a
page fault needs memory machinery we haven't built. But **vector 3, the
breakpoint (`#BP`)**, is special: it's meant to be handled and resumed. The
`int3` instruction raises it deliberately. Our handler prints a line and simply
returns; `iretq` resumes at the instruction after `int3`, and the kernel keeps
running.

That's why `kernel_main` fires an `int3` on purpose: if the machine prints
"survived the breakpoint" and continues, the *entire* path — IDT built correctly,
stub layout correct, dispatch correct, `iretq` correct — is proven end to end.
It's the milestone's built-in self-test.

## Decoding a page fault

When a page fault (vector 14) fires, two pieces of evidence come with it:

- **CR2**, a control register, holds the exact memory address that was accessed.
  We read it with `mov rax, cr2`.
- The **error code** is a bitfield: bit 0 = was the page present (protection
  violation) vs. not present; bit 1 = was it a write; bit 2 = was it a user-mode
  access; and so on.

`describe_page_fault` in the Rust turns those bits into a sentence like "write to
a non-present page" — which, once we have paging, will usually point straight at
the bug.

## How to watch it happen

Because a fault can still be opaque, these are the tools that make it visible
(also in the README):

- `make debug` + `make gdb` — freeze the machine at reset and single-step, or
  break at `interrupt_dispatch` and inspect the `InterruptContext`.
- QEMU's `-d int,cpu_reset` — logs every interrupt, its vector, and the full CPU
  state at any reset. A triple fault becomes a readable trail of faults.

## Where this goes next

- **Milestone 5** assigns a vector to the keyboard and finally enables hardware
  interrupts (`sti`) — until now we've kept them off, so only exceptions fire.
- Robustness upgrade (later): give the double-fault handler its own known-good
  stack via the **IST (Interrupt Stack Table)** and a **TSS**, so even a stack
  overflow can be reported instead of triple-faulting. We deliberately skipped
  that here to keep the first IDT readable; the `ist` field is already present in
  our entry, wired to 0, waiting for it.
