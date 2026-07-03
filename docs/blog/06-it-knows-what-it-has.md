# It knows what it has now

*Milestone 6 — physical memory management. The kernel finally learns how much
RAM the machine has, and hands it out one 4 KiB frame at a time.*

> New to this? Read [docs/concepts/physical-memory.md](../concepts/physical-memory.md)
> alongside — it explains Multiboot2, frames, and why "just start using RAM" is
> a trap, all from scratch.

Milestone 5 taught the kernel to *listen*. This one teaches it something more
basic that we'd been faking all along: it had no idea what memory it owned. Every
address it had touched — `0xb8000` for VGA, `0x100000` for its own code — was
hardcoded. Ask it "how much RAM is there, and which of it is real?" and it had no
answer. Milestone 6 is the answer: parse the map GRUB left us, and stand up a
frame allocator over the usable parts.

### Milestone 6 — physical memory — checklist

Think
- [x] office-hours — below
- [x] scope-guard — below

Plan
- [x] eng-plan — [docs/planning/milestone-06-eng-plan.md](../planning/milestone-06-eng-plan.md)

Build
- [x] Working, demoable state; `make` clean; header check passes
- [x] No new warnings; new `unsafe` justified (all reads unaligned-safe, SAFETY on each)

Review
- [x] kernel-review over the diff (three-lens adversarial pass)

Security
- [x] No new attack surface (still ring 0, no data boundary) — /red-team N/A

Reflect
- [x] document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand:** how a freshly-booted kernel discovers its
   own RAM — that it must be *told* by the bootloader, and why it can't just
   probe — and how it then tracks which physical 4 KiB frames are free.
2. **"Done" as observable behavior:** over serial, the kernel dumps the real
   memory map, reports how much usable RAM it found, then allocates a few frames,
   frees one, and gets the *same* frame back on the next allocation.
3. **Smallest version that teaches it:** a bitmap allocator (one bit per frame)
   over the Multiboot2 map. No paging, no heap, no allocator-of-allocators — just
   "which physical frames are taken."
4. **Working vs. accidentally working:** the reclaim test is the honesty check.
   A bump allocator would pass "alloc hands out distinct frames" too — but only a
   real allocator returns a freed frame on the next `alloc`. If reclaim works,
   it's the real thing.
5. **What could stall this for days:** handing out a frame that's secretly in use
   (the kernel's own image, the Multiboot2 structure, frame 0) — a catastrophic,
   *delayed* bug where something later overwrites live data far from the cause.
6. **Scope check:** below.

## Scope-guard

Verdict: **Hold.** Physical memory management is load-bearing — paging (M7) and
the heap (M8) literally cannot exist without it. No reduction needed; the design
is already the minimum that reclaims. The one thing explicitly *not* built:
touching frames above the 1 GiB identity map (the allocator may *track* them, but
using them waits for paging). Kept in scope but deferred by keeping QEMU at
`-m 128M`.

## Eng-plan (summary)

Two one-concept modules, plus prerequisite plumbing:

- `src/multiboot.rs` — a read-only Multiboot2 parser. Walks the tag list and
  yields raw `(base, length, kind)` regions. A *dumb reader*: no policy.
- `src/frame_allocator.rs` — a bitmap allocator: one bit per 4 KiB frame,
  `0 = free`, `1 = used`, in a 128 KiB `.bss` array covering a fixed 4 GiB.
  `alloc()` scans for the first free bit; `free()` clears one. Chosen over a bump
  allocator *because the milestone's whole point is reclaim.*
- Prerequisites: linker `kernel_start`/`kernel_end` symbols to carve the kernel
  out of usable RAM, and `kernel_main(multiboot_info_addr: u64)` to receive the
  pointer.

The full plan, including the edge-case list and verification oracle, is in
[docs/planning/milestone-06-eng-plan.md](../planning/milestone-06-eng-plan.md).

## What got built

- `src/multiboot.rs` and `src/frame_allocator.rs` as planned.
- `linker.ld` gained `kernel_start` (at the 1 MiB load address) and a
  4 KiB-aligned `kernel_end` after `.bss` — the `.bss` that already holds the
  boot page tables and stack, so they fall safely inside the reserved range.
- `kernel_main` grew its argument, dumps the map, then calls
  `frame_allocator::init` and a self-test.
- The `Makefile` headless boot test now asserts a second marker,
  `M6: frame allocator online`, alongside the long-mode marker.

The allocator's `init` is defensive in three layers: mark everything used, free
only the frames *fully inside* a usable region (base rounded up, end down), then
re-reserve frame 0, `[kernel_start, kernel_end)`, and the Multiboot2 structure's
own footprint.

## Verification status (honest)

For the first time, this was **boot-tested live on the author's own machine**,
not just in CI — the macOS-native UEFI boot path (a separate yak-shave, see
[docs/POST_UPDATE_SETUP.md](../POST_UPDATE_SETUP.md)) landed just in time. The
serial log shows:

```
[ok] physical memory: 121 MiB usable across 8 regions (31171 frames)
[ok] frame allocator: 31079 frames free after reserving kernel + boot info
M6: frame allocator online
[ok] alloc: handed out 0x000000001000, 0x000000002000, 0x000000003000
[ok] reclaim: freed 0x000000002000 and got it back on the next alloc
[ok] rejects double free and out-of-range free
```

`alloc` starting at `0x1000` (not `0x0`) is frame 0 correctly held back as null.
The reclaim line is the one that matters: it proves the bitmap, not a bump.

## Retro

**Plan vs. reality.** The plan held up well — the bitmap design, the module
split, the three-layer `init`, and the edge-case list all survived contact. The
build-order was right too, with one happy subtraction: the plan's step to add a
Multiboot2 header *info-request* tag turned out unnecessary. The plan had flagged
this as an open question ("dump the tags first"), and the dump answered it — GRUB
supplies the memory-map tag by default. The genuinely *naive* spot: the plan's
memory-map "oracle" (usable below 640 KiB, one hole at `0xA0000`, usable from
1 MiB up) was a **SeaBIOS** map. We now boot under **UEFI/edk2**, which fragments
RAM into eight usable regions and ~121 MiB usable, not a tidy ~127. The plan's
exact-boundary asserts would have failed — but the plan had also said "read the
boundaries off the first serial dump, don't trust the doc's numbers," which is
exactly what saved it.

**What broke, and for how long.** Refreshingly little. No triple-fault hunt, no
silent reboot. Each slice — linker symbols, the signature, the parser, the
allocator — built and booted on the first real try. The honest number is "a few
hours, mostly writing and reviewing, near-zero debugging." The reason is the next
two points.

**What earned its keep.** Two things. First, a **parallel research fan-out**
before writing any code: four agents established ground truth — the house style,
the linker layout, the exact byte format, and (the big one) whether the boot
pointer even reached Rust. That last one is the story: a `mov edi, ebx` committed
back in Milestone 1, *with a comment naming Milestone 6*, meant the pointer was
already sitting in `RDI`. The step I'd braced for — threading a pointer through
two mode switches in assembly — was already done. Second, a **temporary map dump
in `kernel_main`**: printing the raw regions *before* writing the allocator, so
the allocator was built against the memory map the machine actually has, not the
one the doc guessed.

**The assumption that cost the most.** That the eng-plan's memory map matched the
machine. It didn't — it was written for the firmware we no longer boot. Cheap
here (the plan hedged), but the lede is real: *your plan's memory map is for a
firmware you might not be running.*

**One thing to do differently.** The three bugs the review caught — an unclamped
iterator that could read past mapped memory into a pre-IDT triple-fault, and two
zero-size infinite loops — were all *malformed-input* cases. The eng-plan's
edge-case list was thorough about *data* edges (frame 0, alignment, `entry_size`
≠ 24) but silent on *adversarial structure* (a corrupt `size` field). Next parser
I write, the eng-plan gets a "malformed/hostile input" row up front, so those
are designed in, not caught in review.

## Takeaway for the next person

A kernel doesn't *find* its memory — it gets *handed* a map and has to trust it,
carefully. The whole job is bookkeeping with catastrophic stakes: give out one
frame that's secretly in use and something dies later, far from the scene. The
bitmap is the most honest tool for it because it's *literally a picture* of which
pages are taken — and the moment a freed frame comes back on the next `alloc`,
you know you built an allocator and not just a counter that only goes up.

Next: **frames → pages.** Milestone 7 turns on paging, and its very first act is
to ask this allocator for frames to hold the page tables.
