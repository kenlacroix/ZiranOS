# Milestone 13 — retro

Honest post-mortem, written right after M13 reached done. The point (PLAN §9) is
the truthful story, not "it works now" — including the parts that are
uncomfortable to admit.

## 1. What actually happened vs. the eng-plan?

The eng-plan was right about almost everything, and that is the whole story of
this milestone. It called the single hardest risk (a triple fault on the ring
3 → ring 0 transition with no TSS/RSP0, office-hours Q5), the U/S-down-the-whole-
walk gotcha (§3.1d), the "working vs. accidentally working" gate (assert
`CS & 3 == 3`, Q4), and the exact build order. Nothing in the plan turned out
naive. The build followed it in five commits, each self-tested and green on the
first or second boot.

Where the plan was *quietly* generous: it treated "fabricate an iretq frame" and
"rewrite the interrupt frame to return to ring 0" as one-liners in the data-flow
diagram. They are correct one-liners, but each hides a precise stack-shape
contract (push order, 16-byte alignment, where `kernel_rsp` is stashed relative
to the callee-saved pushes) that would have been a genuine multi-evening bug hunt
to discover empirically. Here they were *derived from the SDM up front* and
written once, correctly. That is a different kind of work than the M4–M9
milestones, where the debugging *was* the milestone.

## 2. What broke, and how long until I understood why?

Almost nothing broke, and that is the honest and slightly unsatisfying headline.
There was no multi-day page-fault hunt. The two would-be showstoppers — the
triple-fault-on-bad-RSP0 and the user page #PF-ing on its first fetch because an
intermediate table lacked U/S — never happened, because the eng-plan had already
named them and the code was written to avoid them from the start. The GDT swap,
the CS reload via far return, the `iretq` into ring 3, and the frame-rewrite
return all worked essentially first try, verified by serial + QEMU `-d int`.

This is worth being upfront about: this milestone was AI-assisted, and the shape
of the work changed accordingly. The value did not come from *finding* a bug over
two evenings; it came from *front-loading* the reasoning so the bugs never
compiled. The "how long to understand why" number for M13 is close to zero — but
only because the understanding moved to the front, into the eng-plan and the
concept doc, instead of being extracted from a crash.

## 3. Root cause / fix of the things that *did* surface

Two real (small) issues, both caught by review rather than by a crash:

- `map_user_page` OR-s U/S into shared upper tables *as it descends*, but only
  discovers a huge-page conflict at the PD level — so a user VA below 1 GiB would
  loosen `PML4[0]`/`PDPT[0]` and *then* return an error, leaving shared entries
  loosened for a failed call. Fix: assert the VA is above the 1 GiB identity
  window, making the misuse impossible instead of merely unlikely.
- `self_test` treated *any* ring-3 fault as a clean round-trip, so a blob that
  spuriously #PF'd on its first instruction would still print "userspace online."
  Fix: assert the excursion ended via `SYS_EXIT` (violation record stayed 0).

Neither was a machine crash; both were "this would lie to you under a different
input," which is the failure mode that matters once there are no more obvious
ones.

## 4. What earned its keep

QEMU `-d int,cpu_reset` — again. Being able to read `v=80 e=0000 cpl=3
IP=001b:...` straight out of the interrupt log is what turned "the serial says
CPL 3" into *proof*: the emulator itself reporting the vector, the error code, the
CPL, the CS selector, and CR2. For a privilege milestone where the entire claim
is "the CPU really was in ring 3," that log is the primary evidence, not a
secondary check. Also: ending every red-team blob in `SYS_EXIT` so a *non*-
faulting attack reports cleanly instead of hanging — a small harness choice that
made the sgdt/sidt leak legible in one boot.

## 5. The assumption that cost the most (the lede)

Not time — but the assumption most worth flagging is subtler and it's about the
*process*: the temptation to call a privilege boundary "done" because the happy
path printed a `Z`. The office-hours Q4 gate ("it tried to cheat and the hardware
said no, from CPL 3") is the thing that separates a real boundary from a
decoration, and it would have been easy to skip. The enforcement test and then
the red-team pass — 9 escape attempts, all blocked, plus the one leak that *isn't*
— are what make the claim honest. The lede for the post: **a boundary you only
tested from the inside is not a boundary you've tested.**

## 6. One thing to do differently next milestone

For M15, invert the order: write the *attack* first. M13 built the boundary and
then attacked it; M15's entire point is the attack (capture the flag), so the
red-team blob should be the acceptance test the implementation is built against,
not a pass done afterward. The M13 red-team already found the two seams to aim at
(the syscall pointer path, the sgdt/sidt leak) — start M15 there.
