---
name: milestone-office-hours
description: Interrogate a milestone before writing any code. Use at the start of a new milestone (or when an idea feels fuzzy) to reframe it with forcing questions, surface the real learning goal, and decide what "done" concretely means. Adapted from gstack's /office-hours.
---

# milestone-office-hours

Product interrogation, retargeted from "what should we ship" to "what should we
*understand*." Run this before opening an editor on a new milestone. The goal is
to replace a vague intention ("do interrupts next") with a sharp, testable
target and a stated learning objective.

Adapted from gstack's `/office-hours` (github.com/garrytan/gstack, MIT).

## When to use

- Starting a milestone from PLAN.md §5.
- An idea for a feature or refactor that isn't yet concrete.
- You notice you're coding without a clear definition of "done."

## The six forcing questions

Ask each, out loud, and write the answers into the milestone's notes or the
blog draft. Do not skip one because the answer seems obvious — the obvious
answer is often wrong for a kernel.

1. **What is the single thing I will understand after this that I don't now?**
   (One sentence. If there are several, the milestone is too big — split it.)
2. **What does "done" look like as an observable behavior?** Name the exact
   thing you'll see on screen / serial / in GDB. "Catches a page fault and
   prints the faulting address" — not "memory works."
3. **What's the smallest version that still teaches the thing?** A frame
   allocator that only tracks one region still teaches the memory map.
4. **How will I know it's actually working and not accidentally working?**
   (A kernel loves to look fine while being subtly wrong — see paging.)
5. **What is most likely to make this stall for days?** Name the failure mode
   now (triple fault? silent paging bug?) and the tool you'll reach for.
6. **Is any of this drifting toward a non-goal?** If yes, invoke `/scope-guard`.

## Output

A short block at the top of the milestone's blog draft (`docs/blog/`) capturing:
the learning goal (Q1), the concrete done-condition (Q2/Q4), the minimal cut
(Q3), and the named risk + mitigation (Q5). This becomes the post's spine.

Then proceed to `/eng-plan`.
