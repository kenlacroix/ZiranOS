# The illusion of memory

*Milestone 7 — paging / virtual memory. The kernel stops using the map the
bootloader handed it and starts drawing its own.*

> New to this? Read [docs/concepts/virtual-memory.md](../concepts/virtual-memory.md)
> alongside — it explains the 4-level page walk and why the kernel builds its own
> tables, from scratch.

Milestone 6 taught the kernel what physical memory exists. This one adds the
indirection every later abstraction leans on: **virtual memory**. The CPU stops
addressing RAM directly — every address is now translated through page tables the
kernel builds and controls. We start on the flat identity map `boot/boot.asm` set
up just to reach long mode, build our own 4-level tables from Milestone 6 frames,
verify them, and switch `CR3` to them without the machine falling over.

### Milestone 7 — paging — checklist

Think
- [x] office-hours — below
- [x] scope-guard — below

Plan
- [x] eng-plan — [docs/planning/milestone-07-eng-plan.md](../planning/milestone-07-eng-plan.md)

Build
- [x] Working, demoable state; `make` clean; header check passes
- [x] No new warnings; new `unsafe`/asm justified (CR3/invlpg options verified)

Review
- [x] kernel-review — 3-lens adversarial pass over the diff

Security
- [x] No new attack surface (still ring 0, one address space, no data boundary) — /red-team N/A

Reflect
- [x] document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand.** How the CPU turns a virtual address into a
   physical one by walking a 4-level page table — and how the kernel builds and
   installs those tables itself, so *it* decides what maps where, instead of
   living inside the flat identity map `boot/boot.asm` set up to reach long mode.

2. **"Done" as observable behavior.** Over serial:
   - `translate(v)` walks our tables and prints the physical address a virtual
     one resolves to — the page walk, made visible.
   - `map_page(v, f)` takes a free frame from the M6 allocator and maps it at a
     virtual address **above the 1 GiB boot identity map** (memory the kernel
     could not touch before). Write a sentinel through `v`, read it back through
     the frame's identity address, and confirm they alias the same physical RAM.
   - The stretch/climax: switch `CR3` to a page table the kernel built itself and
     **keep running** — the marker `M7: paging online` printed *after* the switch
     proves the kernel correctly mapped its own code, stack, and VGA.

3. **The smallest version that still teaches it.** A `paging` module with
   `map_page`, `unmap_page`, and `translate`, walking and allocating the
   intermediate tables from the M6 frame allocator. The intermediate table frames
   are physical addresses below 1 GiB, so the boot identity map still lets us
   write into them while we build. Proving one fresh mapping above 1 GiB round-
   trips data is the whole lesson. Rebuilding a complete address space and
   switching `CR3` is the natural next beat — real, but separable and riskier.

4. **Working vs. accidentally working.** Paging's signature trap: it looks fine
   while being subtly wrong. Guards:
   - Map to a virtual address the identity map does **not** already cover (above
     1 GiB). A successful read/write there can *only* come from our new tables —
     if we tested an already-mapped address, success would prove nothing.
   - Cross-check `translate(v)` against the frame we mapped, and confirm the
     sentinel written via `v` is visible at the frame's physical (identity)
     address — proving `v` and `f` alias, not just that `v` happens to be writable.
   - Deliberately touch an unmapped address and confirm the M4 `#PF` handler
     reports the right faulting address (`CR2`) — a fault that lands where we
     aimed it is stronger evidence than one that never fires.

5. **What most likely stalls this for days.** PLAN.md §6 already flags it: paging
   bugs are *silent and delayed*. Named failure modes and the tool for each:
   - Switching `CR3` to tables that don't map the current instruction pointer,
     stack, or GDT → instant triple fault, silent reboot, no message. Reach for
     `make debug` (GDB) and QEMU's monitor `info mem` / `info tlb` to dump the
     tables *before* trusting the switch.
   - The classic missing `& !0xfff`: page-table entries pack flags in the low 12
     bits; forget to mask them off when reading the next level's physical address
     and the walk wanders into garbage.
   - Changing a mapping but forgetting `invlpg` → the TLB serves a stale
     translation and the bug looks intermittent.

6. **Drifting toward a non-goal?** Paging is core kernel, not a non-goal — but the
   temptation is to "do it properly": W^X / NX-bit enforcement, guard pages as a
   security feature, or per-process address spaces. NX/W^X is *security hardening*
   (an explicit non-goal); separate address spaces belong to userspace (M13). Keep
   M7 flags minimal — present + writable — and defer the rest.

## Scope-guard

Verdict: **Hold**, flags kept deliberately minimal. `PRESENT | WRITABLE` only, one
address space, no demand paging. The identity map — rather than a higher-half
layout — is the minimal teachable choice; higher-half and recursive mapping are
their own later milestones.

## Eng-plan (summary)

A hand-rolled `src/paging.rs` (no `x86_64` crate — raw `u64`, like the rest of the
kernel) that:
- **identity-maps** physical memory, so `phys_to_virt` is the identity function.
  Because all RAM is under 126 MiB, a physical frame — including a page-table frame
  we just allocated — is reachable at its own address, and the walk reads as pure
  page-table logic with no offset or recursion machinery in the way.
- builds its own PML4 / PDPT / PD (2 MiB huge pages) from Milestone 6 frames,
  **verifies the new tables in-code before switching**, then loads `CR3`.
- exposes `translate`, `map_page` (fallible), and `unmap_page`.

The build order was chosen to de-risk the one genuinely dangerous instruction,
`mov cr3`: prove the read path (`translate`) over the *existing* boot tables first,
then build and verify the new tables while still on the known-good map, and only
then switch. Full plan in
[docs/planning/milestone-07-eng-plan.md](../planning/milestone-07-eng-plan.md).

## What got built

- `src/paging.rs`: `translate` (the 4-level walk, with 2 MiB huge-page handling),
  `build_address_space` + `init` (allocate and zero tables, identity-map
  `[0, 1 GiB)`, verify, switch `CR3`), and `map_page` / `unmap_page`.
- `kernel_main` calls `paging::init()` then `paging::self_test()` after the IDT is
  installed and before `sti`.
- The `Makefile` headless boot test now asserts `M7: paging enabled`.

## Verification status (honest)

Boot-tested live under QEMU. The serial log:

```
M7: paging enabled -- running on kernel-built page tables
[ok] paging: mapped 0x000040000000 -> 0x000000004000, round-tripped 0xdeadbeef above the 1 GiB map
[ok] paging: unmapped 0x000040000000
[ok] paging: self-test left 2 intermediate tables mapped (expected)
```

The marker prints *after* the `CR3` switch — surviving it is the proof the kernel
mapped its own code, stack, GDT, and IDT. The round-trip is at `0x4000_0000`, one
byte past the identity window, so the write landing in the right physical frame
can *only* mean our new tables worked.

## Retro

**Plan vs. reality.** The eng-plan held up almost exactly. The identity-map
decision paid off — `map_page` and `translate` really do read like the textbook
description of a page walk, because `phys_to_virt` is a no-op. One thing I did
*better* than the plan: it said "inspect the new tables in GDB before trusting the
switch"; instead I made that a permanent in-code check — `init` walks the new
tables with `translate_from` and asserts they map correctly *while still on the
boot map*, so a bug panics safely instead of triple-faulting. Automated beats
manual.

**What broke, and for how long.** Nothing — and that's the interesting part.
`mov cr3`, the instruction most likely to silently triple-fault and reboot with no
message, worked on the first try. The honest number is near-zero debugging, and it
wasn't luck: verifying the new map *before* the switch, while still on the
known-good boot tables, converts a silent hardware triple-fault into a loud,
recoverable software panic. The scariest step was de-fanged by construction.

**The technique worth stealing.** Paging is famous for silent triple-faults
because people load `CR3` and hope. Don't. Walk the new tables in software first
(you're still on the old, working map), assert the addresses you're about to
depend on — instruction pointer, stack, GDT, IDT — resolve correctly, and only
then switch. A triple fault gives you nothing; a failed assertion gives you a
message and a live machine.

**The assumption that cost the most (the lede).** Almost nothing cost time during
the build — but the review found the safety net I'd written didn't exist.
`phys_to_virt`'s guard was a `debug_assert!`, and this kernel builds `--release`,
where `debug_assert!` compiles to nothing. A guard you can't see fire is a guard
you can't trust. For a kernel that ships its release profile, invariants that must
hold get a real `assert!`.

**One thing to do differently.** Decide an API's fallibility up front. `map_page`
inherited a panic from `init`'s frame allocation, but a *runtime* map — which the
Milestone 8 heap will do as it grows — has to handle running out of frames. The
review is where the return type became `Result`; it should have been the signature
I wrote first. Next module: ask "who calls this at runtime, and can it fail?"
before writing the type.

## Takeaway for the next person

Virtual memory is one idea — *put a translation table between the CPU and RAM* —
and once it's on, the kernel decides what every address means. Building the tables
is mechanical; the danger is the handoff, the instant you point `CR3` at your own
work and the CPU trusts it for the very next instruction. The craft of doing it
safely is refusing to take that on faith: prove the new map is right while the old
one is still holding you up. Next: **pages → heap.** Milestone 8 puts `Vec`,
`Box`, and `String` on top of these mappings.
