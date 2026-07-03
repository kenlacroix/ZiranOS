---
name: eng-plan
description: Lock in the technical approach for a milestone before building — data flow, the boot/CPU-state assumptions it depends on, edge cases, and how it will be verified. Use after milestone-office-hours and before writing kernel code. Adapted from gstack's /plan-eng-review.
---

# eng-plan

Architecture lock-in for one milestone. gstack's `/plan-eng-review` produces a
data-flow diagram, edge cases, and a test matrix; the kernel version adds the
thing that bites hardest at this level — the **machine-state preconditions** a
piece of code silently assumes (is paging on? which mode? interrupts enabled?
which stack?).

Adapted from gstack's `/plan-eng-review` (github.com/garrytan/gstack, MIT).

## When to use

After `/milestone-office-hours` has defined the target, before building.

## Produce

1. **Approach** — the design in a few sentences. What data structures, what
   registers/tables/hardware, where it lives (asm vs Rust, which module).
2. **Preconditions** — the CPU/memory state this code assumes on entry and
   guarantees on exit. Long mode? Identity-mapped? IDT loaded? A wrong
   assumption here is the classic silent fault. Be explicit.
3. **Data flow** — how control and data move through it, including the handoff
   boundaries (asm→Rust, ISR→handler, allocator→caller).
4. **Edge cases** — the ones that actually occur in kernels: unaligned or
   out-of-range addresses, off-by-one at page/table boundaries, the zero case,
   re-entrancy (an interrupt firing mid-operation), exhaustion (no free frames).
5. **Verification plan** — how you'll *prove* it works, not just observe it not
   crashing. Concretely: what serial output, what GDB check (`info registers`,
   examine a page table entry), what deliberately-triggered failure should be
   caught. Tie this back to office-hours Q4 ("working vs accidentally working").

## Output

A short plan block in the milestone notes / blog draft. Keep it honest about
unknowns — "not sure whether the TLB needs flushing here; will check in GDB" is
a better plan entry than false confidence. Then build.
