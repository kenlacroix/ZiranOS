# The illusion of memory

*Milestone 7 — paging / virtual memory. Draft in progress.*

> Draft: office-hours below; scope-guard, eng-plan, build, and retro fill in as
> the milestone proceeds.

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
   M7 flags minimal — present + writable — and defer the rest. Full `/scope-guard`
   next.
