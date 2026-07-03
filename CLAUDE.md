# CLAUDE.md — how AI-assisted work runs on Ziran OS

Guidance for Claude Code (and compatible agents) working in this repo.

## What this project is

A hobbyist, from-scratch x86_64 OS. Read [PLAN.md](PLAN.md) for the vision and
the milestone roadmap, and [STATUS.md](STATUS.md) for current state, before
starting substantive work. The stack is `no_std` Rust (`x86_64-unknown-none`) +
hand-written Multiboot2/long-mode assembly; the build is spelled out in the
`Makefile`.

## Guardrails (do not violate without an explicit decision in PLAN.md)

- **Honor the non-goals.** No POSIX, no networking (beyond the stretch stub), no
  security hardening, no multi-user, no real-hardware support beyond QEMU. The
  biggest project risk is scope creep toward "real" features — see `/scope-guard`.
- **Keep the boot handoff readable.** No build magic that hides how bytes become
  a bootable image. The `Makefile` pipeline is the documentation.
- **`unsafe` and assembly carry their weight.** Every `unsafe` block and every
  MMIO/`asm!` access states why it is sound (see the existing modules for the
  house style). Volatile for anything memory-mapped.
- **Debugging is the hard part, not an afterthought.** A bad kernel triple-faults
  and silently reboots. Reach for `/investigate` and `make debug` (GDB stub)
  rather than guessing.

## Workflow (adapted from gstack)

This repo adopts a trimmed version of the gstack sprint loop, mapped onto OS
milestones. Full gstack has 23 skills aimed at shipping products; a solo
bare-metal OS only needs the parts below. Each milestone flows:

**Think → Plan → Build → Debug → Review → Reflect**

| Phase | Skill | Output |
|-------|-------|--------|
| Think | `/milestone-office-hours` | Forcing questions that sharpen the milestone before any code |
| Think | `/scope-guard` | A scope check against the non-goals — catches creep early |
| Plan | `/eng-plan` | Architecture, edge cases, and a test/verification plan |
| Build | (normal editing) | Working, demoable state per PLAN.md §5 |
| Debug | `/investigate` | Hypothesis-driven root-cause loop (stop after 3 failed attempts and reassess) |
| Review | `/kernel-review` | Staff-engineer audit tuned for `unsafe`/asm/kernel invariants |
| Reflect | `/retro` | What broke, how long it took, the actual fix — feeds the blog |
| Reflect | `/document-milestone` | Update STATUS.md and draft the milestone's blog post |

There is also a conditional security skill, `/red-team` (adapted from gstack's
`/cso`): adversarial self-testing of the kernel's own boundaries, to *learn* what
they enforce — **not** to harden. It only applies once a boundary exists (ring-3
userspace/syscalls, or a filesystem; PLAN.md §8). Skip it with a one-line reason
otherwise.

**This loop is mandatory for every milestone**, not per-taste. The mechanical
checklist lives in [docs/MILESTONE_CHECKLIST.md](docs/MILESTONE_CHECKLIST.md) —
copy its block into each milestone's blog draft and work through it. That is how
a milestone is considered done here.

Skills live in `.claude/skills/` and travel with the repo — no global install
needed. Invoke them by name (e.g. `/investigate`).

## Attribution

The workflow and the seven skills in `.claude/skills/` are adapted, with thanks,
from **gstack** by Garry Tan (https://github.com/garrytan/gstack, MIT) — a
Claude Code "software factory." They have been rewritten for the bare-metal OS
domain; the product/design/deploy/web skills in gstack were intentionally left
out as out-of-scope for this project.
