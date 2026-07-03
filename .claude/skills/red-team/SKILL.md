---
name: red-team
description: Adversarially test the kernel's own boundaries to learn what they actually enforce — privilege (ring 3 vs ring 0), the syscall interface, and filesystem bounds. Use only once such a boundary exists (userspace/syscalls or a filesystem — PLAN.md milestones 13/11–12). Authorized self-testing of your own code on your own hardware. Adapted from gstack's /cso.
---

# red-team

gstack's `/cso` does OWASP/STRIDE threat modeling for web apps. This is the
bare-metal analogue, and its purpose is different: not to *harden* the OS
(production hardening is a non-goal, PLAN.md §1) but to **break your own kernel
in order to understand its boundaries**. Every attack is a probe: it names an
invariant the system claims to hold, tries to violate it, and reports what
actually happened. "The boundary held, and here's the mechanism that stopped me"
is a successful, valuable result — often the most instructive one.

Adapted from gstack's `/cso` (github.com/garrytan/gstack, MIT).

## Precondition — do not run before there's a boundary

There is nothing to attack while everything runs in ring 0. This skill applies
only once the kernel has:
- a **privilege boundary** — ring-3 userspace + a syscall interface (milestone 13), or
- **persistent data / a parser** — a filesystem (milestones 11–12).

If neither exists yet, say so and stop. (Attacking a single-ring kernel is just
calling functions.)

## Scope and authorization

This is authorized testing of the author's own OS, running in QEMU or on
hardware the author owns. In scope: attacking *this* kernel's own boundaries. Out
of scope: anything targeting systems, networks, or software the author doesn't
own — that's not what this is for.

## The loop, per boundary

For each boundary under test, work through:

1. **Name the boundary and its invariant.** e.g. "ring 3 cannot read pages marked
   supervisor-only," or "a syscall never dereferences a user pointer without
   validating it maps to that process."
2. **State the attack as a hypothesis.** The specific thing you'll do and the
   invariant it would violate. ("A user process passes a kernel address as the
   buffer to `write`; if unchecked, the kernel copies from its own memory.")
3. **Predict the two outcomes.** What you'll observe if the boundary *holds*
   (which fault, caught where) versus if it *leaks* (what data appears where it
   shouldn't).
4. **Run it.** Smallest possible test program / input.
5. **Record the result and the mechanism.** Held or leaked, and *why* — the page
   permission bit, the ring check, the missing bounds check. This is the lesson.
6. **If it leaked:** note it as a finding with a concrete repro. Fixing it is
   optional (learning, not hardening) — but understanding it is not.

## Boundaries worth probing (as they come online)

- **Privilege (ring 3 → ring 0):** privileged instructions (`cli`, `hlt`, control
  register writes) from userspace; reads/writes of supervisor pages; direct jumps
  into kernel code; port I/O without permission.
- **Syscall interface:** out-of-range and unmapped pointers, integer overflow in
  length/offset arguments, TOCTOU on shared buffers, invalid syscall numbers —
  the "confused deputy" surface.
- **Filesystem / parser:** corrupt on-disk structures, off-by-one at extent
  boundaries, path handling, reads past a file's end into adjacent data.

## Output

Per boundary: the invariant, the attacks tried, and for each — held or leaked,
plus the mechanism. Feed the interesting ones (both the solid boundaries and the
leaks) into the milestone's blog post via `/retro` + `/document-milestone`.
