# Milestone 8 — heap allocator — eng-plan

Locks the technical approach before building. Grounded in a three-way research
fan-out (the `no_std` `alloc` ceremony on stable, the integration points, and the
allocator design). Companion concept doc: `docs/concepts/heap.md` (written during
the build).

## 0. Scope-guard (do this first)

The heap is core kernel. The traps:

- **One honest allocator, not a production one.** A linked-list free-list, not a
  buddy or slab allocator with coalescing — those solve performance problems we
  can't measure yet. First-fit, no coalescing of adjacent frees, is the documented
  teaching minimum (the "later refinement" hook, like `unmap_page`'s deferred
  table reclaim).
- **No bump allocator.** It makes `Vec`/`Box` *look* like they work while leaking
  every reallocation. M6 rejected bump for the frame allocator "because a real OS
  must reclaim"; the heap gets the same honesty. (A throwaway bump is fine as a
  build-order stepping stone to isolate the linker ceremony — see §6 — but the
  shipped allocator reclaims.)
- **No demand paging.** Map the whole heap region eagerly at init. Lazy fault-in
  would need the M7 `#PF` handler to allocate-and-retry instead of halting — real
  interrupt-return plumbing that belongs to a later milestone.
- **No new dependencies.** Hand-roll the allocator in the `spin`-only, no-`x86_64`
  house style; do **not** pull in the `linked_list_allocator` crate.
- **One kernel heap.** No per-process heaps / userspace `malloc` (that's M13).

Verdict: **Hold**, deliberately minimal.

## 1. Approach

A new `src/heap.rs` plus the `alloc`-crate wiring in `lib.rs`:

- `lib.rs` gains `extern crate alloc;` (at the crate root), `mod heap;`, and a
  `#[global_allocator]`.
- `heap::init()` reserves a fixed virtual region and **eagerly maps every page**
  onto a fresh M6 frame via `paging::map_page`, then hands the contiguous region
  to the allocator as one initial free block. Called from `kernel_main` after
  `paging::init()`/`self_test()` and before `pic::init()`.
- The allocator is a **linked-list free-list**: each free region stores a
  `ListNode { size, next }` *inside its own memory* (no side table — the intrusive
  free-list idea M6 couldn't use for frames, now viable because M7 gave us mapped
  memory to store the links in). `alloc` is first-fit with splitting and
  alignment; `dealloc` pushes the block back onto the list.
- `GlobalAlloc` takes `&self`, so the list lives behind `spin::Mutex`, wrapped in
  a `LockedHeap` newtype (the orphan rule blocks `impl GlobalAlloc for Mutex<…>`).

Constants:
```
HEAP_BASE = 0x0000_0000_4000_0000   // 1 GiB — first byte past the identity map
HEAP_SIZE = 0x10_0000               // 1 MiB = 256 pages
PAGE_SIZE = 4096
```
Base at 1 GiB is deliberate: it's exactly where the identity map ends (M7's
`init` asserts `translate(0x4000_0000) == None`) and the address M7's self-test
already proved a frame maps and round-trips at. A heap pointer there can *only*
resolve through the kernel's own page tables — the honesty is built in.

**Stable-Rust ceremony (verified, current):** `extern crate alloc;` is required
(edition 2021 still needs it in `no_std`); **no `#![feature(...)]`** and **no
`#[alloc_error_handler]`** — since Rust 1.68 the default handler turns an OOM into
`panic!("memory allocation of N bytes failed")`, which routes through our existing
`#[panic_handler]`. `panic = "abort"` + staticlib means `#[global_allocator]`
generates the `__rust_alloc` symbols inside `libziran_kernel.a`; GNU `ld` resolves
them, no linker flags. This ceremony *was* the pre-1.68 pain PLAN.md remembers; on
our pinned stable it's clean.

## 2. Preconditions

**On entry** (`heap::init` called from `kernel_main`, after `paging::self_test`,
before `sti`):
- Long mode; running on the kernel's own page tables (M7 switched CR3), which
  identity-map `[0, 1 GiB)` and leave everything ≥ 1 GiB unmapped.
- The M6 frame allocator and M7 `map_page` are both up (the heap needs both).
- Interrupts disabled — the heap self-test allocates, and nothing should preempt.

**On exit:** `[HEAP_BASE, HEAP_BASE+HEAP_SIZE)` is backed by real frames and owned
by the allocator; `Vec`/`Box`/`String` work for the rest of the kernel's life.

**API change required:** `paging::WRITABLE` is currently module-private. Make it
`pub` so `heap` can request writable pages: `paging::map_page(v, p, paging::WRITABLE)`
(`map_page` OR-s in `PRESENT` itself).

## 3. Data flow

```
kernel_main
  └─ heap::init()
       ├─ for each of 256 pages in [HEAP_BASE, HEAP_BASE+HEAP_SIZE):
       │     frame = frame_allocator::alloc()          (panic on exhaustion)
       │     paging::map_page(virt, frame, WRITABLE)    (panic on failure)
       ├─ ALLOCATOR.lock().init(HEAP_BASE, HEAP_SIZE)   one initial free region
       └─ serial "M8: kernel heap online"
  └─ heap::self_test()   // Box, Vec-past-capacity, String, alloc/drop/alloc reuse

Vec::push / Box::new  ->  __rust_alloc  ->  <LockedHeap as GlobalAlloc>::alloc
   -> lock the Mutex -> first-fit find_region -> split -> return aligned ptr (or null)
drop  ->  __rust_dealloc  ->  ::dealloc  -> add_free_region (reclaim)
```
The 256 pages all fall in `[1 GiB, 1 GiB+2 MiB)`, sharing one PD and one PT. M7's
self-test already mapped-then-leaked exactly those two intermediate tables (it
mapped `0x4000_0000`, and `unmap_page` clears only the leaf), so `heap::init`
finds them present and allocates just the **256 heap frames** — M7's documented
"leak" is quietly reused here. (If the tables weren't there, `map_page` would
create them; init works either way.)

## 4. Edge cases

- **OOM at runtime:** `alloc` returns **null** (never panics inside `alloc`); the
  container calls `handle_alloc_error` → default panic → our `#[panic_handler]`.
- **OOM during init:** `frame_allocator::alloc` or `map_page` returns
  `None`/`Err` → panic (foundational, one-time init must succeed, like
  `paging::init`).
- **Alignment:** `align_up(addr, align) = (addr + align - 1) & !(align - 1)`, valid
  because `layout.align()` is always a power of two. The returned pointer **must**
  be ≥ `layout.align()`-aligned or it's UB.
- **Minimum block size:** a freed block must be able to hold a `ListNode`, so round
  every allocation up to at least `size_of/align_of::<ListNode>()` — otherwise
  `dealloc` can't re-thread the region.
- **Split remainder too small:** if aligning + carving `size` leaves a remainder
  smaller than a `ListNode`, that region can't be split cleanly — reject it and
  try the next region (first-fit continues), rather than create an untrackable
  sliver.
- **Zero-size allocation:** the `GlobalAlloc` contract guarantees `alloc` is never
  called with a zero-size `Layout` (containers use a dangling pointer instead), so
  we don't handle it — but note the assumption.
- **Fragmentation:** first-fit without coalescing means adjacent frees don't merge;
  fragmentation can accumulate. Documented limitation, the natural later refinement.
- **Re-entrancy / deadlock:** the heap lock is a `spin::Mutex`; safe today
  (single core, no allocation from an interrupt handler). If a future ISR
  allocates while the lock is held → deadlock. Note it; the M9 revisit, same as
  the frame allocator and paging.

## 5. Verification plan (working vs. accidentally working)

Over serial, mirroring the frame-allocator/paging self-tests:

1. **`Box::new` works** — the simplest heap allocation resolves at all.
2. **`Vec` pushed past its initial capacity** — forces a real reallocation (grow,
   move, free the old buffer) *through* the allocator. A bump allocator would
   silently leak every dropped intermediate buffer here.
3. **`String` grows** — heap-backed dynamic text.
4. **The honesty check — reclaim:** allocate a `Box`, record its address, drop it,
   allocate again, and assert the **same address** comes back. This is the direct
   analogue of the frame allocator's reclaim test ("the proof it's a real bitmap
   and not a bump allocator") lifted to the heap — and the one thing a bump
   allocator provably *cannot* do (its cursor only advances).
5. **(Optional) multi-page span:** allocate a buffer larger than 4 KiB, fill and
   read it back, proving the whole region is mapped, not just the first page.
6. CI marker `M8: kernel heap online`; the Makefile `run-headless` grep updated
   from the M7 marker to this one.

## 6. Build order (each its own small commit, QEMU-tested)

De-risks by isolating the linker/`alloc` ceremony (the historical time-sink) from
the allocator algorithm:

1. **Ceremony + eager heap mapping + a throwaway bump allocator.** Make
   `paging::WRITABLE` public; add `extern crate alloc`, `mod heap`, the
   `#[global_allocator]`; `heap::init` maps the region; a trivial bump allocator.
   Verify: it *links* (the `__rust_alloc` symbols resolve — the risky part), a
   `Box`/`Vec` allocate, and `M8: kernel heap online` prints. This proves the
   ceremony and the mapping independently of allocator correctness.
2. **Replace bump with the linked-list free-list.** `ListNode`, `find_region`
   (first-fit + align + split), `add_free_region`; the reclaim self-test
   (alloc/drop/alloc address reuse) passes.
3. CI marker in the Makefile; `heap::self_test` wired into `kernel_main`;
   `docs/concepts/heap.md`; the `web/` M8 tour step. Then `/kernel-review` →
   `/retro` → `/document-milestone`.

## 7. Open unknowns (honest)

- Exact `Vec` growth pattern (how many reallocations 256 pushes trigger) — will
  read it off the serial log; only matters for the leak-would-show demo.
- Whether the first-fit split ever hits the "remainder too small for a ListNode"
  branch with the self-test's allocation sizes — will log if it does.
- Whether to keep the throwaway bump commit in history or squash it into step 2 —
  decide at commit time (leaning keep, as honest de-risking, like M7's translate
  step).
