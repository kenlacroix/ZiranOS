# Room to grow

*Milestone 8 — the heap allocator. Draft in progress.*

> Draft: office-hours below; scope-guard, eng-plan, build, and retro fill in as
> the milestone proceeds.

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
   Full `/scope-guard` next.
