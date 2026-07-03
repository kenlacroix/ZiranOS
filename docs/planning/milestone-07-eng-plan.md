# Milestone 7 — paging / virtual memory — eng-plan

Locks the technical approach before building. Grounded in a four-way research
fan-out (paging mechanics, the existing boot tables, integration points, and the
page-table-access design tradeoff). Companion concept doc:
`docs/concepts/virtual-memory.md` (written during the build).

## 0. Scope-guard (do this first)

Paging is core kernel, not a non-goal — but "do it properly" is the trap:

- **Flags: `PRESENT | WRITABLE` only.** No NX bit (that needs `EFER.NXE` and is
  W^X *security hardening* — an explicit non-goal). No `USER` bit — there is no
  ring 3 yet (userspace is M13).
- **One address space.** No per-process page tables, no CR3-per-task. That is
  userspace/scheduling territory (M9/M13).
- **No demand paging / swapping / CoW.** There is no disk to page to.
- **Identity map, not higher-half.** Moving the kernel to the higher half and
  mapping all RAM at an offset is a real, separate milestone; it drags in linker
  relocation and is orthogonal to the paging lesson. Deferred.

Verdict: **Hold**, with the flag set deliberately minimal.

## 1. Approach

A new hand-rolled `src/paging.rs` (no `x86_64` crate — raw `u64`, matching the
house style), built on the M6 frame allocator. It:

1. Builds its **own** 4-level page-table hierarchy (PML4 → PDPT → PD → PT),
   allocating each table frame from `frame_allocator::alloc()` and **zeroing it**
   (alloc does not zero).
2. **Identity-maps** physical memory `[0, IDENTITY_LIMIT)` (1 GiB, via 2 MiB huge
   pages — same shape the boot code uses, cheap: 1 PML4 + 1 PDPT + 1 PD). This
   covers every physical address the kernel touches (image, VGA `0xb8000`, stack,
   GDT, IDT, the M6 bitmap, and every allocator-sourced table frame — all
   `< 126 MiB`). Under identity mapping `phys_to_virt(p) == p`, so a table frame
   is edited directly through its own physical address.
3. Switches `CR3` to the new PML4 and keeps running.
4. Exposes `translate(virt) -> Option<u64>`, `map_page(virt, phys, flags)`, and
   `unmap_page(virt)`, each a plain 4-level walk (4 KiB pages).

Chosen design: **identity map**, because `phys_to_virt` is the identity function,
so the walk reads as pure page-table logic with zero incidental machinery; it
reproduces the map the bootloader already gave us (honest continuity across the
switch); and with ≤126 MiB RAM its only weakness — virtual-space exhaustion — is
moot. Recursive mapping and the higher-half offset map are deferred as their own
later milestones.

Key constants (from the paging reference):
```
FRAME_SIZE      = 4096
ADDR_MASK       = 0x000F_FFFF_FFFF_F000   // bits 51..12; note: NOT just !0xfff
PRESENT         = 1 << 0
WRITABLE        = 1 << 1
HUGE            = 1 << 7                   // PS, at the PD level = 2 MiB page
IDENTITY_LIMIT  = 1 << 30                  // 1 GiB
index(v, level) = (v >> (12 + 9*level)) & 0x1FF   // level 3=PML4 .. 0=PT
```
CR3/`invlpg` access via inline `asm!` mirroring `interrupts::read_cr2`; `mov cr3`
and `invlpg` must **not** carry `nomem` (they change how memory is seen).

## 2. Preconditions

**On entry** (`paging::init` is called from `kernel_main` after
`frame_allocator::init` + `interrupts::init`, and **before `sti`**):
- Long mode active; CR3 = the boot identity map of `[0, 1 GiB)` (2 MiB huge pages).
- The M6 frame allocator is initialized (paging depends on it for table frames).
- The IDT is loaded (so a mapping bug surfaces as a reported #PF, not a mystery).
- **Interrupts are disabled** (we run before `sti`), so no ISR fires mid-switch.

**On exit:**
- CR3 = our own PML4, which identity-maps `[0, 1 GiB)` — every region the kernel
  uses is mapped, so execution continues seamlessly.
- `map_page` / `unmap_page` / `translate` are available for M8 (heap) onward.

## 3. Data flow

```
kernel_main
  └─ paging::init()
       ├─ pml4 = alloc()+zero
       ├─ identity-map [0,1GiB): alloc pdpt+pd, fill PD with 2MiB huge entries,
       │    pdpt[0]->pd, pml4[0]->pdpt        (edited via phys_to_virt = identity)
       ├─ write_cr3(pml4)                     ← the dangerous instant
       └─ serial "M7: paging enabled"         ← printed only if the switch survived
  └─ paging::self_test()
       ├─ f = alloc()
       ├─ map_page(TEST_VIRT, f, PRESENT|WRITABLE)   TEST_VIRT above 1 GiB
       ├─ *(TEST_VIRT as *mut u32) = 0xDEADBEEF
       ├─ assert *(f.start_address() as *const u32) == 0xDEADBEEF   (alias proof)
       ├─ assert translate(TEST_VIRT) == f.start_address()          (walk proof)
       ├─ unmap_page(TEST_VIRT); assert translate(TEST_VIRT) == None
       └─ free(f)
```
Handoff boundaries: allocator→paging (raw `PhysFrame`, must be zeroed by paging);
paging→CPU (`mov cr3`); the walk edits tables through the identity alias.

## 4. Edge cases

- **Un-zeroed table frame** — `alloc` returns dirty memory; a stale bit reads as a
  present entry pointing at a garbage frame. **Every** freshly-allocated table
  frame is zeroed (512 × `u64` = 0) before any entry is written.
- **The address mask** — use `ADDR_MASK` (`0x000F_FFFF_FFFF_F000`), not `!0xfff`;
  a plain `!0xfff` leaves NX/available/reserved high bits in the next-level
  address and the walk wanders into garbage.
- **CR3 switch with something unmapped** → instant silent triple-fault. Mitigation:
  the identity map provably covers RIP/stack/GDT/IDT/tables (all `< 1 GiB`); verify
  the new tables with GDB `info mem` / by `translate`-ing known addresses *before*
  trusting the switch (see build order — the switch is step 3, after translate is
  proven).
- **`map_page` into a huge-mapped region** ( `[0,1GiB)` ) — would require splitting
  a 2 MiB page into 4 KiB; not supported. The test maps **above** 1 GiB, where the
  PDPT entry is absent, so `map_page` creates fresh PDPT/PD/PT. Assert (return an
  error / `None`) if a walk hits an unexpected `HUGE` entry.
- **Unaligned virtual address** to `map_page` — round down to the 4 KiB page (or
  assert alignment).
- **Non-canonical test address** — keep `TEST_VIRT` canonical (low half,
  `< 0x0000_8000_0000_0000`); e.g. `0x0000_0000_4000_0000` (exactly 1 GiB) is the
  first address just past the identity window.
- **Exhaustion** — `alloc` returns `None` while building tables → `paging::init`
  cannot build the address space; panic with a clear message (like M6's no-map
  case).
- **TLB staleness** — `invlpg [virt]` after `unmap_page` and after remapping an
  existing page. A brand-new mapping into a not-present entry does not strictly
  require it, but we `invlpg` anyway for safety.
- **Re-entrancy** — not handled (runs before `sti`, never from an ISR). Note it;
  a lock comes with M9, same as the M6 allocator.

## 5. Verification plan (working vs. accidentally working)

The office-hours honesty check: paging can look fine while subtly wrong.

1. **The switch survives.** `M7: paging enabled` is printed *after* `mov cr3`. If
   the new tables failed to map the code/stack/GDT/IDT, the machine would have
   triple-faulted at the switch instead. Surviving *is* the proof the kernel
   mapped itself. → CI marker (Makefile `run-headless` greps for it).
2. **Alias round-trip above the identity window.** Map frame `f` at `TEST_VIRT`
   (≥ 1 GiB, *not* covered by the identity map), write `0xDEADBEEF` through
   `TEST_VIRT`, read it back through `f`'s identity address, assert equal. Success
   can *only* come from our new mapping — testing an already-identity-mapped
   address would prove nothing. This is the Q4 guard.
3. **Walk cross-check.** `translate(TEST_VIRT) == f.start_address()`.
4. **Unmapped is detected.** `unmap_page(TEST_VIRT)`, then
   `translate(TEST_VIRT) == None` — the software walk correctly reports "no
   mapping." (A *hardware* #PF demo is deliberately **not** run in the normal boot:
   the M4 #PF handler halts, which would stop the boot before the M5 keyboard. It
   stays available as a GDB/opt-in exercise; noted, not wired in.)
5. **GDB, during bring-up.** `info registers cr3` before/after the switch; QEMU
   monitor `info mem` to dump the new tables; examine a PML4/PDPT/PD/PT entry to
   confirm flags + masked physical address.

## 6. Build order (each its own small commit, QEMU-tested)

De-risks the scary CR3 switch by proving the read path first:

1. `src/paging.rs`: constants, `phys_to_virt` (+ `debug_assert < IDENTITY_LIMIT`),
   `read_cr3`/`write_cr3`, entry compose/mask helpers, and **`translate`** — a
   read-only walk over the *existing boot tables* (CR3 unchanged). Verify:
   `translate(0xb8000) == 0xb8000`, `translate(kernel_start) == kernel_start`,
   `translate(1 GiB) == None`. This proves the walk with zero risk.
2. Build the new PML4 + identity map, but **don't switch CR3 yet**. Verify by
   walking the *new* PML4 (pass its frame to a translate variant) and comparing to
   the boot map; inspect in GDB.
3. **Switch CR3.** Verify the kernel survives and prints `M7: paging enabled`.
4. `map_page` / `unmap_page`; the alias round-trip + translate + unmap self-test.
5. CI marker in the Makefile; `paging::self_test()` wired into `kernel_main`
   (after `interrupts::init`, before `sti`); `docs/concepts/virtual-memory.md`;
   the `web/` M7 tour step. Then `/kernel-review` → `/retro` → `/document-milestone`.

## 7. Open unknowns (honest)

- Whether a brand-new not-present→present mapping needs `invlpg` on this QEMU/TCG
  build — will `invlpg` regardless and note if removing it changes anything.
- Exact QEMU-monitor incantation to dump 4-level tables (`info mem` vs `info tlb`)
  — confirm during build.
- Whether to keep the bulk identity map as 2 MiB huge pages (planned) or drop to
  4 KiB — huge pages are cheaper and match boot; 4 KiB only if a walk needs it.
