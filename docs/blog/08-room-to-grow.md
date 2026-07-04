# Room to grow

*Milestone 8 — the heap allocator. The kernel gains dynamic memory: `Vec`, `Box`,
and `String` start working with no operating system underneath.*

> New to this? Read [docs/concepts/heap.md](../concepts/heap.md) alongside — it
> explains what a heap is, how `Vec` gets its memory, and the free-list allocator,
> from scratch.

Everything the kernel has allocated so far was fixed-size and known at compile
time — the stack, the static page tables, the frame bitmap. This milestone adds
the thing that lets data structures *grow at runtime*: a **heap**, and with it
Rust's `alloc` crate. It sits directly on the last two milestones — a virtual
region whose pages M7 maps onto M6 frames — completing the stack the memory work
has been building: **frames → pages → heap.**

### Milestone 8 — heap — checklist

Think
- [x] office-hours — below
- [x] scope-guard — below

Plan
- [x] eng-plan — [docs/planning/milestone-08-eng-plan.md](../planning/milestone-08-eng-plan.md)

Build
- [x] Working, demoable state; `make` clean; header check passes
- [x] No new warnings; new `unsafe` justified (the free-list's raw-pointer nodes)

Review
- [x] kernel-review — 3-lens adversarial pass over the diff

Security
- [x] No new attack surface (still ring 0, one heap, no data boundary) — /red-team N/A

Reflect
- [x] document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand.** How `Vec`, `Box`, and `String` actually get
   their memory on bare metal — that they all bottom out in one `GlobalAlloc`
   trait the kernel implements, backed by a region of virtual memory the kernel
   mapped for itself. This is the milestone where `alloc` — dynamic memory —
   becomes available with no OS underneath.

2. **"Done" as observable behavior.** Over serial: push to a `Vec` until it
   *reallocates* (grows past its initial capacity) and print its contents;
   allocate a `Box<T>` and a `String` and print them; and — the honest one — free
   and re-allocate and show the memory come back. A marker `M8: heap online`, then
   `[ok] Vec grew to N; Box and String work; freed memory was reused`.

3. **The smallest version that still teaches it.** A fixed-size heap region (a
   megabyte or so) mapped **eagerly** at init via M7's `map_page` onto M6 frames,
   plus a `#[global_allocator]`. The real teaching content is the *plumbing* —
   `extern crate alloc`, the `GlobalAlloc` impl, the alloc-error handler, and the
   heap-on-mapped-pages wiring — not a fancy algorithm. The open question for the
   eng-plan: a **bump** allocator (trivial, but leaks on every free) vs. a simple
   **free-list** (reclaims). M6 rejected a bump allocator precisely because we want
   reclaim; the heap probably deserves the same honesty.

4. **Working vs. accidentally working.** A bump allocator makes `Vec::push` "work"
   — it grows — while leaking every reallocation, so "Vec works" is *not* proof of
   a real allocator. Guards:
   - Allocate, `drop`, allocate again, and show the second allocation **reuses the
     freed address**. That's what separates an allocator from a bump pointer.
   - Force an allocation whose `Layout` demands over-alignment (e.g. 64 bytes) and
     confirm the returned pointer is actually aligned — a `GlobalAlloc` that
     ignores `align` looks fine until something needs it.
   - Allocate a buffer that **spans several pages** and read it all back, proving
     the whole heap region is mapped, not just its first page.

5. **What most likely stalls this for days.** PLAN.md's own words: *"getting `Vec`
   to work felt bigger than it should."* The `no_std` + `alloc` ceremony on stable
   Rust: `extern crate alloc`, `#[global_allocator]`, and especially the
   **alloc-error handler** (historically nightly-only via `#[alloc_error_handler]`;
   modern stable provides a default — but if the toolchain wants a feature or a
   handler, that's the classic time-sink). And `GlobalAlloc` is `unsafe` returning
   a raw `*mut u8`: a misalignment, off-by-one, or a pointer into unmapped memory
   corrupts the heap and crashes far from the cause — the same silent-and-delayed
   class as paging. Tools: serial-print every alloc address, `make debug` / GDB.

6. **Drifting toward a non-goal?** Heap is core kernel. The temptation is a
   production allocator (buddy/slab with coalescing) — over-engineering; a simple
   free-list is enough. Demand-paging the heap (lazy) is deferred — eager mapping
   is simpler and M7's `#PF` handler halts. Per-process heaps / a userspace
   `malloc` are M13. Keep it: one kernel heap, fixed region, one honest allocator.

## Scope-guard

Verdict: **Hold**, deliberately minimal. A linked-list free-list, not a buddy or
slab allocator with coalescing — those solve performance we can't measure yet.
No bump allocator (it leaks every reallocation; M6 rejected one for the same
reason). Eager mapping, one kernel heap, no new crate dependency — hand-rolled in
the `spin`-only, no-`x86_64` house style.

## Eng-plan (summary)

A new `src/heap.rs` plus the `alloc`-crate wiring in `lib.rs`:
- `heap::init()` reserves a **1 MiB virtual region at `0x4000_0000`** (1 GiB — the
  first byte past the identity map, M7's `map_page` as its first real customer)
  and **eagerly maps** every page onto an M6 frame.
- The allocator is a **linked-list free-list**: each free region stores a
  `ListNode { size, next }` *inside its own bytes* — the intrusive list M6 wanted
  for frames but couldn't use until M7 gave us mapped memory to hold the links.
  first-fit with splitting and alignment; `dealloc` pushes the block back, so it
  *reclaims*.
- Behind `spin::Mutex` in a `LockedHeap` newtype, because `GlobalAlloc` takes
  `&self` and the orphan rule forbids `impl GlobalAlloc for Mutex<…>`.

The build order isolated the historical time-sink (the linker/`alloc` ceremony)
from the algorithm: a throwaway bump allocator first, to prove it *links* and
`Box`/`Vec` work, then the free-list. Full plan in
[docs/planning/milestone-08-eng-plan.md](../planning/milestone-08-eng-plan.md).

## What got built

- `src/heap.rs`: the free-list allocator, `heap::init` (map the region + hand it
  to the allocator), and `heap::self_test`.
- `src/lib.rs`: `extern crate alloc;`, `mod heap;`, and — that's the whole
  ceremony on stable.
- `src/paging.rs`: `WRITABLE` made public so the heap can map writable pages.
- The `Makefile` headless boot test now asserts `M8: kernel heap online`.

## Verification status (honest)

Boot-tested live under QEMU. The serial log:

```
[ok] heap: 1024 KiB mapped at 0x000040000000 (256 pages)
M8: kernel heap online
[ok] heap: Box, Vec (grown), String, and a 4-page Vec all allocate
[ok] heap: freed 0x000040000ff0 and got the same address back (reclaim works)
```

The `Vec` is pushed past its capacity (forcing a real reallocation), the 4-page
`Vec` proves the whole region is mapped, and the reclaim line is the honest one:
free an allocation, allocate again, get the **same address** back. A bump
allocator would return a fresh, higher address and fail that assert — which is
exactly why it was rejected.

## Retro

**Plan vs. reality.** The eng-plan was accurate and the build-order de-risking
worked: the ceremony *linked on the first try*, isolated from the allocator by a
throwaway bump. The one naive spot the review caught: the plan's frame accounting
was off by two — it reasoned about M8 in isolation and forgot that M7's self-test
had already leaked exactly the PD+PT for `0x4000_0000` that the heap then reuses,
so init costs 256 frames, not 258.

**What broke, and for how long.** Nothing. The ceremony linked, the bump worked,
the free-list worked, reclaim passed. Near-zero debugging — and that's the
milestone's real lesson.

**The assumption worth re-examining (the lede).** PLAN.md tags this milestone
*"getting `Vec` to work felt bigger than it should."* That reputation is a
**stale artifact of the nightly era.** Before Rust 1.68 a `no_std` build that used
`alloc` failed to link without a hand-written `#[alloc_error_handler]`, gated
behind a nightly feature — *that* was the pain. On the pinned stable toolchain the
research fan-out found the whole ceremony is now two lines: `extern crate alloc;`
and `#[global_allocator]`. An OOM just panics through the panic handler we already
have. A milestone's fearsome reputation can be a version-old ghost; checking
beats budgeting for it.

**What earned its keep.** The research fan-out, again — one agent's cited answer
(the default alloc-error handler, stable since 1.68) meant I never wrote the
doomed nightly ceremony. And the build-order split: proving the linker/`alloc`
plumbing with a trivial bump allocator meant that when I added ~150 lines of
free-list, any new failure could *only* be the algorithm, not the plumbing.

**One thing to do differently.** When a milestone builds directly on the previous
one's memory, trace the previous one's *actual* end-state, not its intended one —
M7's deliberate two-table leak changed M8's frame math, and the review shouldn't
be where that coupling first surfaces.

## Takeaway for the next person

A heap is just a pool of memory you mapped, plus a bookkeeper that hands out and
takes back pieces of it. `Vec`, `Box`, and `String` aren't magic — they all call
one trait, `GlobalAlloc`, and on bare metal *you* are the implementation, over
memory *you* mapped onto frames *you* found. The honest test that you built a real
allocator and not a leak is a single line: free something, allocate again, and get
the same address back. Next: **the memory stack is complete** (frames → pages →
heap), and Milestone 9 — a timer and the first taste of scheduling — starts
building on top of it.
