---
name: kernel-review
description: Staff-engineer code review tuned for kernel work — unsafe soundness, assembly correctness, MMIO/volatile, and the machine-state invariants that make bare-metal bugs silent. Use before committing a milestone or any change touching unsafe/asm/hardware. Adapted from gstack's /review.
---

# kernel-review

gstack's `/review` is a staff-engineer audit that auto-fixes the obvious and
flags the rest. This version keeps that stance but points it at the things that
bite in ring 0, where the compiler's usual guarantees stop at the `unsafe`
boundary and a mistake reboots the machine instead of throwing.

Adapted from gstack's `/review` (github.com/garrytan/gstack, MIT).

## When to use

Before committing a milestone; on any diff touching `unsafe`, assembly, page
tables, MMIO, port I/O, interrupt handlers, or linker/build config.

## Checklist (in priority order)

**Soundness of `unsafe`**
- Does every `unsafe` block have a comment stating *why* it is sound? Is the
  claim actually true?
- Raw pointers: valid, aligned, non-dangling, and pointing where intended?
- Memory-mapped I/O uses `read_volatile`/`write_volatile` — never a plain deref
  the compiler may cache, reorder, or elide.

**Assembly**
- Registers clobbered by an operation saved/restored as the ABI requires?
- The asm→Rust handoff: correct calling convention, stack alignment, arguments
  in the right registers?
- Any instruction that assumes a CPU mode/state that isn't guaranteed yet?

**Machine-state invariants**
- Preconditions from `/eng-plan` actually established before the code runs
  (paging on, IDT loaded, correct stack, interrupts in the intended state)?
- Interrupt handlers: re-entrancy safe? Not calling anything that can't run in
  interrupt context (e.g. taking a lock the interrupted code already holds —
  the deadlock risk in our spinlock-guarded writers)?

**Correctness at boundaries**
- Page/frame math at 4 KiB / 2 MiB boundaries; off-by-one on table indices.
- Exhaustion and zero cases handled, not assumed away.

**Fit**
- Does the change respect the non-goals (`/scope-guard`) and match the house
  comment style — explaining *why*, at the altitude of the surrounding code?

## Output

Findings grouped by severity, most dangerous first. Fix the clear-cut ones
directly; for anything uncertain, describe the risk and the failure scenario
rather than silently "fixing" code you don't fully understand — in a kernel, a
confident wrong fix is worse than a flagged unknown.
