# Milestone 6 — Physical memory management — eng-plan

*"Learning what memory it has."* Produced with the `/eng-plan` skill
(.claude/skills/eng-plan). Locks in the approach before any kernel code. Sections
follow the skill: Approach · Preconditions · Data flow · Edge cases ·
Verification. Kept honest about unknowns — see the ⚠️ notes.

**Goal (STATUS.md M6):** consume the Multiboot2 memory map (pointer already in
RDI at the `kernel_main` handoff), learn which physical regions are usable RAM,
and build a frame allocator that **hands out and reclaims** 4 KiB physical frames,
skipping the regions the kernel and boot structures occupy.

---

## 0. Prerequisite changes (must land first — M6 can't work without them)

These are the two gaps found while reading the current boot path, plus the ABI
seam. Do them before the allocator, each as its own small, verifiable commit.

1. **Request the memory-map tag in the Multiboot2 header**
   (`boot/multiboot_header.asm`). Today the header is only magic + end tag; GRUB
   *usually* provides the memory map (tag 6) unrequested, but that's luck, not
   contract. Add an **information-request tag** (type 1) listing tag 4 (basic
   meminfo) and tag 6 (memory map).
   - ⚠️ Gotcha: every tag must be 8-byte aligned, so the info-request tag (type
     u16, flags u16, size u32, then a u32 per requested type) needs a padding
     dword if the request count is odd. The header checksum is computed from
     `header_end - header_start`, so it self-adjusts when the header grows —
     don't hand-patch it.
   - Verify: `make check-header` still passes (magic intact); boot still reaches
     long mode.

2. **Export kernel image bounds from the linker** (`linker.ld`). Add
   `kernel_start = .;` immediately before `.boot` (at `. = 1M`) and
   `kernel_end = .;` after `.bss`. The allocator reads these as `extern "C"`
   statics to avoid handing out any frame the kernel itself occupies.
   - ⚠️ These symbols carry *addresses*, not values: in Rust,
     `extern "C" { static kernel_start: u8; }` and take `&kernel_start as *const _
     as u64`. Reading the byte would be meaningless.

3. **Grow the `kernel_main` signature to receive the pointer.** boot.asm already
   stashes EBX→EDI→RDI, and long_mode_init.asm's comment says it "arrives as
   kernel_main's first C-ABI argument *if/when the Rust signature grows one*."
   Change to `pub extern "C" fn kernel_main(multiboot_info_addr: u64) -> !`.
   - ⚠️ RDI is the SysV first integer arg — correct. But confirm nothing between
     `_start` and the `call` clobbers RDI. boot.asm sets EDI early then calls
     check_multiboot / check_cpuid / check_long_mode / set_up_page_tables /
     enable_paging. Those are our own routines and don't touch EDI/RDI, but
     **verify in GDB** (`info registers rdi` at the `kernel_main` breakpoint) —
     a clobber here is a classic silent "why is my pointer garbage" bug.

---

## 1. Approach

Two new flat modules, matching the one-concept-per-file house style:

- **`src/multiboot.rs`** — a minimal, read-only Multiboot2 boot-information
  parser. Just enough to walk the tag list and expose the memory-map entries as
  an iterator of `(base, length, kind)`. No allocation — it borrows the
  GRUB-provided structure in place (it's in identity-mapped low RAM).
- **`src/frame_allocator.rs`** — a **bitmap frame allocator**: one bit per 4 KiB
  physical frame, `0 = free`, `1 = used`. `alloc() -> Option<PhysFrame>` scans
  for the first free bit; `free(frame)` clears a bit. A `PhysFrame(u64)` newtype
  (4 KiB-aligned physical address) is the currency.

**Why a bitmap, not a bump allocator.** STATUS.md explicitly wants *reclaim*, and
a bump/`next-free` allocator can't free. A bitmap gives O(1) free, cheap
double-free detection (the bit is already set), and is the most *teachable* shape
for the web tour ("here is literally the map of which pages are taken"). The scan
for alloc is O(n) over words, trivially fast at this scale (128 MiB = 32 Ki
frames = 512 u64 words). An intrusive free-list was rejected: it stores the
next-pointer *inside* the freed frame, which is unsound for frames above the
1 GiB identity map until paging exists (M7).

**Where the bitmap lives.** Statically reserved in `.bss`, sized for a fixed
maximum supported RAM. Default target is QEMU `-m 128M`; reserve enough for
**4 GiB** of coverage = 4 GiB / 4 KiB / 8 = **128 KiB** bitmap. This sidesteps the
bootstrap problem (no allocator needed to place the allocator's own storage) and
is dead simple. RAM beyond the bitmap's range is *not silently ignored* — see
Edge cases.

---

## 2. Preconditions (machine state this code assumes)

| Assumption | Status at M6 entry | Consequence if wrong |
|---|---|---|
| 64-bit long mode active | ✅ guaranteed by boot.asm | n/a |
| First 1 GiB identity-mapped (virt==phys), 2 MiB huge pages | ✅ `set_up_page_tables` | Any pointer we *dereference* (the MB struct, the bitmap in .bss, frames we read) **must be < 1 GiB**. The MB struct and kernel are loaded near 1 MiB — safe. |
| RDI = physical address of Multiboot2 info struct | ✅ boot.asm (needs GDB confirm, §0.3) | Garbage map → allocator hands out live memory |
| Interrupts | `sti` happens in `kernel_main` *after* M5 setup | **Build the allocator before or after `sti`, but it is NOT re-entrant.** Do not call `alloc`/`free` from an interrupt handler yet — no locking. |

**Guarantees on exit:** a global frame allocator is initialised; every usable
frame is marked free *except* frame 0, the kernel image
(`kernel_start..kernel_end`), the Multiboot2 struct
(`mb_addr..mb_addr+total_size`), and the bitmap's own storage if it happens to
sit in a usable region (it's in .bss inside the kernel image, so already covered).

⚠️ **>1 GiB frames:** the allocator may *track and hand out* a frame above 1 GiB
(it's real RAM), but the caller cannot *touch* it until M7 maps it. For M6 keep
QEMU at `-m 128M` (well under 1 GiB) so alloc→use is always safe; note the
limitation loudly in the module doc and the concept doc.

---

## 3. Data flow

```
boot.asm: EBX (MB info phys addr) ─► EDI ─► (survives long-mode switch) ─► RDI
   │
   ▼
kernel_main(multiboot_info_addr: u64)          [src/lib.rs]
   │  after M4/M5 bring-up
   ▼
frame_allocator::init(multiboot_info_addr, kernel_start, kernel_end)
   │
   ├─► multiboot::BootInfo::new(addr)          [src/multiboot.rs]
   │       read total_size (u32 @ +0), skip reserved (u32 @ +4)
   │       walk tags from +8:  {type:u32, size:u32, payload, pad→8B}
   │       stop at end tag (type==0, size==8)
   │       find tag type==6 (memory map): {entry_size:u32, entry_version:u32, entries…}
   │       yield each entry {base_addr:u64, length:u64, kind:u32, _:u32}, stride = entry_size
   │
   ├─► phase 1: mark the WHOLE bitmap "used" (safe default — never hand out the unknown)
   ├─► phase 2: for each entry with kind==1 (usable), mark its 4 KiB frames FREE
   │            (round base UP, end DOWN to frame boundaries)
   └─► phase 3: re-mark RESERVED as used — frame 0, [kernel_start,kernel_end),
                [mb_addr, mb_addr+total_size)
   ▼
FRAME_ALLOCATOR (global)  ──►  alloc() / free(frame)  used by M7 (page tables), M8 (heap)
```

The two handoff seams to watch: **asm→Rust** (RDI, §0.3) and
**parser→allocator** (the parser yields raw entries; the allocator owns the
free/reserved policy — keep that split clean so the parser stays a dumb reader).

---

## 4. Edge cases (the ones that actually bite here)

- **Frame 0 / null.** Never hand out physical 0 — it's indistinguishable from a
  null pointer and lives in the legacy low-memory minefield. Reserve it.
- **Unaligned region bounds.** A usable entry rarely starts/ends on a 4 KiB
  boundary. Round **base up**, **end down**; a partial frame at either edge is
  not fully usable, so drop it. Off-by-one at the boundary = handing out a frame
  that overlaps reserved memory.
- **`entry_size` ≠ 24.** The mmap tag's `entry_size` is authoritative for the
  stride; the spec allows it to exceed the 24-byte struct we know. **Stride by
  `entry_size`, not `size_of::<Entry>()`** — hardcoding 24 is a real corruption
  bug if a loader pads entries.
- **Tag alignment.** Each tag is padded to the next 8-byte boundary; advance by
  `(size + 7) & !7`. Forgetting this desyncs the whole tag walk.
- **The MB struct sits in usable RAM.** GRUB places it in a region the map itself
  reports as usable. If we don't reserve `[mb_addr, mb_addr+total_size)` we'll
  hand out frames holding the very map we parsed. Reserve it (could be reclaimed
  post-parse later; keep it simple now).
- **Kernel overlaps a usable region.** The image at 1 MiB is inside a usable
  entry — carve out `[kernel_start, kernel_end)` explicitly.
- **RAM beyond the bitmap's 4 GiB coverage.** Do **not** silently truncate
  (house rule: no silent caps). Clamp to the bitmap range and `serial_println!`
  exactly how many MiB were ignored, so it's visible not lost.
- **No memory-map tag present.** If the walk never finds tag 6 (e.g. header
  request missing and GRUB didn't volunteer it), `init` must **panic with a
  clear message**, not proceed with an empty map (which looks like "0 usable
  frames" and wedges M7 mysteriously later).
- **Exhaustion.** `alloc()` returns `None` when no bit is free — caller must
  handle, never `unwrap()` blindly. 128 MiB = 32 Ki frames, so exhausting it in
  a real test is impractical; test the path against a tiny synthetic map instead.
- **Double free / freeing a reserved or never-allocated frame.** `free()` should
  detect an already-clear bit and report rather than silently corrupt the count.
- **Re-entrancy.** Not handled at M6 (single-threaded, not called from ISRs).
  When M9 (scheduling) or interrupt-driven allocation arrives, wrap the bitmap in
  a spinlock. Note it; don't build it yet.

---

## 5. Verification plan (prove it, don't just watch it not crash)

Tie-back to office-hours Q4 "working vs *accidentally* working": the bar is that
the reported map **matches QEMU's known layout** and that alloc/free behave
*exactly*, not just "it printed some numbers."

1. **Dump the parsed map over serial.** For each entry: base, length, kind.
   Then a summary line, e.g. `usable RAM: 127 MiB across 3 regions (32512 frames)`.
   - **Oracle:** QEMU `-m 128M` has a *known* e820-ish layout — usable below
     ~640 KiB, a hole `0xA0000–0xFFFFF` (VGA/BIOS), then usable from 1 MiB up to
     just under 128 MiB. Assert the dump shows that shape and the total ≈ 128 MiB
     minus the holes. A map that doesn't match this is parsing garbage.
2. **Allocation invariants.** Allocate ~8 frames, print addresses. Assert each is
   4 KiB-aligned, distinct, inside a usable region, and outside
   `[kernel_start, kernel_end)` and the MB struct. (Cheap to check inline.)
3. **Reclaim actually works.** `alloc` → record F → `free(F)` → `alloc` again →
   assert you get **F back**. This is the one test that separates a real bitmap
   from a bump allocator wearing a costume.
4. **Failure paths, deliberately triggered** (office-hours Q4):
   - Run `init` against a *synthetic* one-region map with N frames, alloc N+1,
     assert the last returns `None` (exhaustion).
   - `free()` a frame that's already free → assert it reports the double-free.
5. **GDB checks.** Breakpoint `kernel_main`: `info registers rdi` == the address
   GRUB passed (cross-check against QEMU's `-d` output). Examine the bitmap in
   memory after init to eyeball the used/free pattern around 1 MiB.
6. **CI marker.** Add a stable serial line (e.g. `M6: frame allocator online`)
   and extend the headless test's grep (`Makefile` `run-headless`, alongside the
   existing `Ziran OS booted` marker) so "still boots *and* still sees its RAM"
   is enforced on every push.
7. **Web tour step.** Per the milestone checklist, add the M6 output line + a
   predict→observe→explain step to `web/`, and a from-first-principles
   `docs/concepts/physical-memory.md`.

⚠️ **Open unknowns to resolve while building (don't fake confidence):**
- Whether GRUB in this setup already supplies tag 6 without the §0.1 request —
  check by dumping tags *before* adding the request; add it regardless for
  contract, but the observation is a good blog beat.
- Exact QEMU `-m 128M` region boundaries — read them off the first serial dump
  rather than trusting this doc's approximate numbers.
- Whether `entry_size` comes back as 24 or larger from this GRUB — log it.

---

## Build order (small, each independently verifiable)

1. §0.2 linker symbols → boots, symbols resolve (`nm build/kernel.bin | grep kernel_`).
2. §0.3 `kernel_main` arg + GDB-confirm RDI.
3. §0.1 header info-request tag → `make check-header` + boot OK.
4. `src/multiboot.rs` parser → serial-dump the map (verification #1).
5. `src/frame_allocator.rs` bitmap + `init` → verification #2–4.
6. CI marker (#6), concept doc + web step (#7), then `/kernel-review` → `/retro`
   → `/document-milestone` per CLAUDE.md.
