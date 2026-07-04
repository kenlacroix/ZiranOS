# Virtual memory, from first principles

*Companion to Milestone 7. Read this before `src/paging.rs` — it explains what
that file is *for* and why "just turn on paging" hides a whole hierarchy of
machinery. Builds directly on [physical-memory.md](physical-memory.md): that doc
gave the kernel a `frame_allocator` that hands out 4 KiB physical frames but
could only *track* frames above the boot map, never touch them. This is the
milestone that lifts that restriction. The one thing you need coming in is the
idea that after `boot/boot.asm` we are in 64-bit long mode with the first 1 GiB
of physical memory identity-mapped.*

---

## The problem: physical addresses are rigid

Milestone 6 left the kernel able to say, of any 4 KiB physical frame, "this one
is free" or "this one is used." That is real progress — but every address it can
name is a *physical* address, a fixed coordinate on the memory bus. And physical
addresses are rigid in a way that becomes a wall:

- **The kernel can't choose what lives where.** The VGA buffer *is* `0xb8000`.
  The kernel image *is* at 1 MiB. If we want a data structure at some convenient
  round virtual address — say, a heap starting at exactly 1 GiB — physical
  addressing has no answer. You get the address the RAM happens to sit at, full
  stop.
- **We can't touch RAM the boot map didn't reach.** This is the wall we hit at
  the end of physical-memory.md. The frame allocator can *record* a frame at
  physical `0x8000_0000` as free, but the moment we dereference that address the
  CPU walks the boot page tables, finds nothing mapped there, and faults. The
  frame is real RAM we own on paper and cannot write a single byte into.
- **Everything shares one flat space.** Later milestones want *isolation* (a
  userspace process that cannot see the kernel's memory) and *relocation* (the
  same code running at different addresses). Neither is expressible when an
  address is just a bus coordinate.

The fix is a layer of **indirection**. Instead of the CPU sending the address in
your instruction straight to the memory bus, it sends it through a translation
step first: a *virtual* address goes in, the hardware looks it up in tables the
kernel controls, and a *physical* address comes out. The kernel writes those
tables, so the kernel decides the mapping. That indirection is virtual memory,
and it is the illusion every later abstraction — the heap, and eventually
separate address spaces — is built on.

## How the hardware translates: the 4-level walk

On x86-64 the translation tables are a tree four levels deep, and the CPU's MMU
walks that tree on every memory access. The input is a 48-bit virtual address
(the low 48 of a 64-bit register; the top 16 bits are just sign-extension, which
is why addresses look like `0xffff_8000_...` or `0x0000_...`). Those 48 bits are
not one number — they are **four 9-bit table indices plus a 12-bit offset**:

```text
  47      39 38      30 29      21 20      12 11        0
  |  PML4   |  PDPT    |   PD     |   PT     |  offset   |
     9 bits    9 bits     9 bits     9 bits    12 bits
```

Nine bits selects one of 512 entries (`2^9 = 512`); twelve bits addresses one of
4096 bytes (`2^12 = 4096`, our frame size). Each table is 512 entries × 8 bytes
= exactly 4 KiB — **one table is one frame.** That is not a coincidence; it is
the whole system closing on itself, and it is why the frame is the atom
everything is built from.

The walk starts at a register, **CR3**, which holds the physical address of the
top table, the **PML4**. From there it is four hops down the tree, each index
picking the next table:

```text
   virtual address
        │
   CR3 ─┴─> PML4[index₃] ──> PDPT[index₂] ──> PD[index₁] ──> PT[index₀] ──> frame
             (level 3)        (level 2)        (level 1)      (level 0)      + offset
```

`index₃` (bits 47:39) picks a PML4 entry, which points at a PDPT. `index₂` picks
a PDPT entry pointing at a PD. `index₁` picks a PD entry pointing at a PT.
`index₀` picks a PT entry, and *that* entry holds the physical frame the page
lives in. Add the 12-bit offset and you have the final physical address. Four
memory reads by the hardware, and the kernel never lifted a finger — it only had
to build the tables once.

In `src/paging.rs` that index extraction is one small function:

```rust
fn table_index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * level)) & 0x1FF) as usize
}
```

Level 3 is the PML4, level 0 the PT. Shift past the 12-bit offset plus `9 *
level` bits, keep the low 9 (`& 0x1FF`). The whole `translate`/`map`/`unmap`
family is just this walk, done for reading or writing.

## What lives in an entry: the page-table entry format

Each entry is a `u64`, and it is doing two jobs at once — pointing at the next
table (or the final frame) *and* carrying permission flags. It gets away with
this because of alignment. Every table and every frame is 4 KiB-aligned, so the
low 12 bits of any physical address they might hold are always zero. Those 12
free bits become **flags**:

```text
  63          52 51                            12 11        0
  | NX / avail  |     physical address (bits 51:12)  |  flags  |
                                                       ┌─┬─┬───┐
                                                       │P│W│...│
                                                       └─┴─┴───┘
                                                        │ └ writable (bit 1)
                                                        └── present  (bit 0)
```

Milestone 7 uses exactly two flag bits:

- **`PRESENT` (bit 0).** The entry is valid. Walk into a not-present entry and
  the hardware raises a page fault (`#PF`). A zeroed entry is not-present — which
  is exactly why fresh table frames must be zeroed (more on that below).
- **`WRITABLE` (bit 1).** `0` = read-only, `1` = writable for the whole region
  under this entry.

That is the entire flag vocabulary for this milestone, deliberately. **NX
(no-execute) and W^X are not here** — NX needs `EFER.NXE` enabled and is
security hardening, an explicit non-goal. **The `USER` bit is not here** — there
is no ring 3 to protect against yet (userspace is a later milestone). Two flags
is the honest minimum that makes paging *work*, and nothing more.

**The mask subtlety, and it is a real corruption bug.** To follow an entry to
the next table you must extract the physical address — bits 51:12 — and *only*
those bits. The tempting mask is `!0xfff` (clear the low 12 flag bits). That is
wrong: it leaves the high bits (NX at 63, the available bits at 62:52) sitting in
what you're about to treat as an address, and the walk descends into garbage.
The correct mask keeps bits 51:12 and clears everything else:

```rust
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000; // bits 51..12 — NOT !0xfff
```

`src/paging.rs` calls this out explicitly in a comment, because it is precisely
the kind of bug that "works" on a small map (where the high bits happen to be
zero) and detonates later.

**Huge pages (the `PS` bit).** One more wrinkle: an entry can *end the walk
early*. If bit 7 (`PS`, "page size") is set at the PD level, that entry does not
point at a PT — it maps a **2 MiB page** directly, and the low 21 bits of the
virtual address become the offset into it. At the PDPT level `PS` means a 1 GiB
page. This is a big deal for us: the bootloader's identity map, and the one we
build in this milestone, both use 2 MiB huge pages — 512 of them cover a whole
gigabyte with a single PD, no PTs at all. So any correct walk must check `PS` and
stop, masking the page's low bits out of the base before adding the offset:

```rust
const HUGE: u64 = 1 << 7; // PS: at the PD level, maps a 2 MiB page directly
```

One sharp edge the code guards: bit 7 is *reserved/ignored* at the PML4 level, so
`translate` only honors `HUGE` at levels ≤ 2 — misreading a stray bit up top as a
bogus "512 GiB page" would send the walk nowhere real.

## Why Ziran identity-maps

There is a policy question buried in all this: when the kernel holds a *physical*
address (a table frame it just allocated, say) and needs to *read or write* it,
what virtual address does it use? The kernel runs on virtual addresses now, so it
cannot poke a physical address directly — it needs a mapping from physical to
virtual, a `phys_to_virt`.

Ziran makes the simplest possible choice: **identity mapping.** `phys_to_virt(p)`
is just `p`. Physical address `0x10_0000` is reachable at virtual `0x10_0000`.
The map is a straight line, `virtual == physical`, over the whole first gigabyte.

This is a choice we can afford *because of a fact from the last milestone*: all
usable RAM on our QEMU machine is well under 126 MiB, and the one device we touch
(VGA at `0xb8000`) is comfortably inside 1 GiB. So a single identity-mapped
window of `[0, 1 GiB)` covers *everything the kernel will ever address* — the
image, the stack, the GDT, the IDT, the M6 bitmap, and every table frame the
allocator can ever hand back. There is nothing outside the window to reach.

The payoff is conceptual clarity. Because `phys_to_virt` is the identity
function, the page-walk code in `src/paging.rs` reads as **pure page-table
logic** — no offset arithmetic, no recursive-slot trickery, no machinery standing
between you and the tree. When the code reads a table at `table_phys`, it just
casts `table_phys` to a pointer and dereferences. That is the whole trick, and it
is why this is the right first paging milestone: the lesson is the walk, not the
addressing scheme wrapped around it. It also gives us honest **continuity** — we
are rebuilding the exact map the bootloader already handed us, so nothing about
what's-reachable changes across the switch.

`IDENTITY_LIMIT` (1 GiB) is enforced, loudly, in `phys_to_virt`:

```rust
fn phys_to_virt(phys: u64) -> u64 {
    assert!(phys < IDENTITY_LIMIT, "paging: physical address outside the identity map");
    phys
}
```

A full `assert!`, not `debug_assert!`, so it survives into the `--release` image:
if a frame ever fell outside the window it would panic *here* rather than
silently forming a wild pointer that corrupts memory and faults somewhere
unrelated. It never fires today; it guards the future >1 GiB machine.

**The landscape we're skipping (on purpose).** Two other classic answers to the
`phys_to_virt` question exist, and it is worth knowing they're there so you know
what we deferred:

- **Recursive mapping** points one PML4 entry back at the PML4 itself, so the
  page tables appear *inside* the virtual address space and can be edited without
  a separate physical-to-virtual map. Clever, but it is machinery in service of a
  problem (reaching table frames) that the identity map dissolves for free.
- **Higher-half + physical-memory offset** maps *all* physical RAM at a high
  virtual base (e.g. `0xffff_8000_...`) and moves the kernel up there, freeing
  the low half for future userspace. This is the grown-up layout — but it drags
  in linker relocation and is orthogonal to learning the walk. It is its own
  milestone, deferred.

Identity mapping is the honest choice for *this* milestone: everything fits, and
nothing gets in the way of seeing the tree.

## Building our own tables, and the CR3 switch

The bootloader already left us a working identity map. So why build another one?
Because the boot tables were assembled by `boot/boot.asm` and live at addresses
the kernel doesn't own or track. To *own* our address space — to be the thing
that adds and removes mappings — we need tables **we** allocated, from **our**
frame allocator. `paging::init` builds them:

1. Allocate three frames from `frame_allocator::alloc()` — one each for the
   PML4, PDPT, and PD. (Huge pages keep it to three: no PTs needed for a 1 GiB
   map.)
2. **Zero every one before writing to it.** This is non-negotiable and easy to
   forget. The M6 allocator does *not* zero the frames it hands out — it returns
   whatever bits were there. An un-zeroed table frame is full of random `u64`s,
   and a random `u64` with bit 0 happening to be set reads as a *present entry
   pointing at a garbage frame*. `zero_frame` writes 512 zeroed `u64`s first, so
   every unwritten entry is a clean not-present `0`.
3. Fill the PD with 512 huge-page entries (`page | PRESENT | WRITABLE | HUGE`),
   entry `i` mapping the 2 MiB page at physical `i * 2 MiB`, and link
   `pdpt[0] -> pd`, `pml4[0] -> pdpt`. That is a complete identity map of
   `[0, 1 GiB)`, in tables we own.

Then comes **the dangerous instant** — loading CR3:

```rust
unsafe fn write_cr3(pml4_phys: u64) { /* mov cr3, {} */ }
```

The moment that instruction retires, the CPU throws away the boot tables and
translates *every* subsequent access — the very next instruction fetch, the
stack push, the next GDT or IDT reference — through the new tables. If the new
tables fail to map even one of those, the fetch faults, the fault handler can't
be fetched either, and the CPU **triple-faults**: on QEMU that is a silent
instant reboot, no message, no core dump. This is exactly the class of bug
CLAUDE.md means by *"debugging is the hard part, not an afterthought"* — a wrong
byte here doesn't throw an error, it vaporizes the machine.

The defense is to **verify before you trust**. While we are still safely running
on the boot map, `init` walks the *new* PML4 with `translate_from` (the same walk
as `translate`, but against a given root instead of live CR3) and asserts a few
representative points — the VGA buffer, the kernel image, an address just under
1 GiB, and that 1 GiB itself is *unmapped*:

```rust
assert_eq!(translate_from(pml4, 0xb8000),     Some(0xb8000),     "new map: VGA buffer");
assert_eq!(translate_from(pml4, 0x10_0000),   Some(0x10_0000),   "new map: kernel image");
assert_eq!(translate_from(pml4, 0x3FFF_F000), Some(0x3FFF_F000), "new map: near 1 GiB");
assert_eq!(translate_from(pml4, 0x4000_0000), None,              "new map: 1 GiB unmapped");
```

Because the map is *uniform*, a handful of points proves the whole window. If any
assert fails, we panic *safely* on the old map instead of triple-faulting on the
new one. Only after the map checks out do we `write_cr3`, and the very next line
prints `M7: paging enabled`. That serial line is not decoration — **the fact that
it prints at all is the proof the kernel successfully mapped itself.** Had the
switch been wrong, the machine would have rebooted before reaching it.

## map, translate, unmap — and the TLB

With our tables live, the kernel gets three operations, each a variation on the
same four-level walk:

- **`translate(virt) -> Option<u64>`** walks the live tables (rooted at CR3) and
  returns the physical address, or `None` if any level along the way is
  not-present. It is read-only — the software mirror of what the hardware does on
  every access.
- **`map_page(virt, phys, flags) -> Result<(), MapError>`** installs a mapping.
  It walks down, and wherever an intermediate table is missing it allocates and
  zeroes a new one from the frame allocator and links it in, finally writing the
  leaf PT entry. Crucially it returns a **`Result`**, not a panic: allocating an
  intermediate table can fail (`MapError::OutOfFrames`), or the target might land
  inside an existing huge page we refuse to split (`MapError::HugePage`). This
  fallibility is deliberate design for the future — the M8 heap will call
  `map_page` repeatedly as it grows and *must* be able to handle a shortage
  gracefully rather than crash. (The one-time `init` build, by contrast, treats
  a frame shortage as unrecoverable and panics — there is no graceful path that
  early.)
- **`unmap_page(virt) -> bool`** clears the leaf PT entry, returning `false` if
  it wasn't mapped. It deliberately leaves intermediate tables in place — a
  small, bounded leak; reclaiming empty tables is a later refinement.

**The TLB, and why a stale translation is a bug.** Walking four tables on every
single memory access would be ruinously slow, so the CPU caches recent
translations in the **Translation Lookaside Buffer**. That cache is a
correctness hazard the moment we *change* a mapping: if we unmap a page but the
TLB still holds its old translation, the CPU will happily keep using the dead
mapping, reading or writing memory that is no longer supposed to be there — a
silent, spooky-action-at-a-distance bug. So after every change to a live
mapping, `paging.rs` issues `invlpg`:

```rust
fn flush_tlb(virt: u64) { /* invlpg [virt] */ }
```

`invlpg` drops the cached translation for one page, forcing the hardware to
re-walk the tables on the next access. Loading CR3 flushes the whole
(non-global) TLB at once, which is why the big switch in `init` needs no explicit
`invlpg`. A subtlety worth internalizing: `invlpg` (and `mov cr3`) must *not* be
marked `nomem` in the `asm!` options — they change how memory is subsequently
seen, and telling the compiler otherwise would let it reorder accesses across the
flush.

## Verification honesty: working vs. accidentally working

Physical-memory.md was careful about the difference between an allocator that
*works* and one that only *looks* like it works. Paging has the same trap, and a
nastier version of it: a `map_page` test can pass for the wrong reason. If you
map some virtual address, write to it, read it back, and it matches — that proves
*nothing* if the address you chose was already identity-mapped. You'd be reading
back your own write through the boot-era mapping, and a completely broken
`map_page` would sail through.

So `self_test` maps a frame at a virtual address **above** the 1 GiB identity
window — `TEST_VIRT = 0x4000_0000`, exactly one gigabyte, the first address past
the map:

```rust
const TEST_VIRT: u64 = 0x4000_0000; // exactly 1 GiB — first address past the map
```

Nothing maps that address until `map_page` does. The test first asserts
`translate(TEST_VIRT) == None` (it really starts unmapped), then maps a fresh
frame there, **writes `0xDEADBEEF` through `TEST_VIRT`, and reads it back through
the frame's own identity address.** If the two agree, the only possible
explanation is that our new mapping aliased `TEST_VIRT` onto that physical frame
— success *cannot* come from a pre-existing mapping, because there wasn't one.
Then it unmaps and confirms `translate` reports `None` again.

There is even honesty about the leak: the test asserts that exactly *two*
intermediate table frames (the PD and PT that `map_page` had to create for the
empty region above 1 GiB) are left behind after unmap, so the deliberate,
bounded leak stays *visible and intentional* rather than drifting silently. That
is the same discipline as M6's "no silent caps" — an accounting surprise is a
bug, so make the expected number explicit and assert it.

## Where this goes next

Paging is the middle stone of the memory stack, and it exists to be built on:

- **Milestone 8 (the heap)** is the first real customer. `Vec`, `Box`, and
  `String` need a heap; a heap needs a big run of usable virtual address space;
  and every page in it is installed by `map_page` and backed by a physical frame
  from `frame_allocator`. This is the moment the "can track but not touch"
  restriction from physical-memory.md finally dies — the heap can live *above*
  the boot map, on frames the allocator knew about all along but couldn't reach
  until these tables existed. `map_page` returning a `Result` is exactly the seam
  the heap needs to grow gracefully.
- **Later milestones** cash in the indirection this milestone bought but didn't
  yet spend: per-process address spaces (a CR3 per task, for isolation), the
  higher-half kernel layout, and the NX / W^X protections we deliberately left
  out here.

That is the shape of the memory stack: **frames (M6) → pages (M7) → heap (M8).**
This milestone lays the middle stone — teaching the kernel, for the first time,
to *choose* what any address means.
