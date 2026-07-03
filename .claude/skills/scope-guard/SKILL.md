---
name: scope-guard
description: Challenge the scope of proposed work against the project's explicit non-goals. Use when a change starts pulling in networking, real filesystems, POSIX-like behavior, security hardening, multi-user, or real-hardware support — the scope-creep risks named in PLAN.md. Adapted from gstack's /plan-ceo-review.
---

# scope-guard

gstack's `/plan-ceo-review` challenges strategic scope in four modes. Here it
does one job: defend the non-goals. The single biggest risk to this project
(PLAN.md §6) is drifting toward "real" features that trade the learning goal for
open-ended engineering. This skill is the deliberate friction against that.

Adapted from gstack's `/plan-ceo-review` (github.com/garrytan/gstack, MIT).

## The non-goals (from PLAN.md §1)

POSIX compliance · real hardware beyond QEMU/common virtualization · a
networking stack (beyond the stretch stub) · security hardening · multi-user ·
anyone but the author using it.

## When to use

Any time a task starts to touch one of those, or when a milestone's plan grows a
"while I'm here…" appendage. Also good before committing to a milestone's scope.

## The four verdicts

Assess the proposed work and return exactly one:

- **Hold** — in scope and correctly sized. Proceed.
- **Reduce** — in scope but too big; name the smaller cut that still hits the
  learning goal (defer the rest to a stretch milestone or a later post).
- **Reframe** — the work is chasing a non-goal, but there's an in-scope version
  that teaches the same thing. State it. (e.g. "not a real FS — a read-only
  FAT16 on a RAM disk, milestone 11.")
- **Reject** — squarely a non-goal with no learning payoff for *this* project.
  Say no, record why, move on.

## Output

One verdict, one paragraph of reasoning tied to a specific non-goal or to the
learning objective, and — for Reduce/Reframe — the concrete smaller target. If
the decision changes project direction, it belongs in PLAN.md, not just chat.
