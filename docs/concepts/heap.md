# The heap, from first principles

*Companion to Milestone 8. Read this before `src/heap.rs` — it explains what that
file is *for* and why "just call `malloc`" hides the fact that on bare metal there
is no `malloc` to call: the kernel has to *be* it. Builds directly on
[physical-memory.md](physical-memory.md) and [virtual-memory.md](virtual-memory.md):
M6 gave us a `frame_allocator` that hands out 4 KiB physical frames, and M7 gave us
`paging` that can map any frame to a virtual address of our choosing. This is the
milestone where those two finally combine into the thing every higher abstraction
quietly assumes exists — a place to put data whose size you don't know until the
program runs. The one thing you need coming in is the picture from the sibling
docs: after `paging::init` we run on the kernel's own page tables, which
identity-map `[0, 1 GiB)` and leave everything above it unmapped.*

---

## The problem: some sizes aren't known until runtime

Up to now, every byte the kernel has touched had its size fixed at compile time.
A `u64` is eight bytes. A `[u8; 80]` line buffer is eighty. The stack grows and
shrinks, but only in units the compiler laid out in advance — one stack frame per
function call, each a known size. Static data in `.bss` and `.data` is the same
story: the linker reserved exactly as much as the source asked for, and not a byte
more.

That covers a surprising amount. But it hits a wall the moment the *amount* of
data depends on something the compiler can't see:

- **A collection that grows.** A `Vec<u64>` that starts empty and gets `push`ed to
  256 times. How many bytes does it need? Nobody knows at compile time — it depends
  on the loop, the input, the runtime. The compiler can't reserve space for "as
  many as it turns out to be."
- **Text built from input.** A `String` assembled from characters typed at a shell
  (a milestone or two away). Its length is whatever the user types.
- **A structure whose shape is decided at runtime** — a tree, a list of open files,
  a table sized to the machine you booted on.

The stack can't hold these: a stack slot's size is fixed when the function is
compiled, and the data outlives the function anyway (return a `Vec` and its buffer
must survive the return). Static memory can't either: `.bss` is sized once, at
link time, for a worst case you'd have to guess. What all three want is memory you
can ask for *in whatever amount you need, whenever you need it, and give back when
you're done* — a pool you carve up at runtime. That pool is the **heap.**

On a hosted program this is invisible. You write `Vec::new()` and it Just Works,
because underneath, the C library's `malloc` (or Rust's global allocator on top of
it) is handing out heap bytes, and the operating system handed *those* to the
process. But we have no operating system underneath us. **We are the operating
system.** There is no `malloc` to fall back on. If `Vec` is going to work in this
kernel, the kernel has to supply the heap itself — reserve the memory, and be the
allocator that parcels it out.

## How `Vec`, `Box`, and `String` get their memory

Here is the piece that makes this tractable: all of Rust's dynamic types funnel
through **one trait**. `Vec`, `Box`, `String`, `BTreeMap` — every one of them,
when it needs memory, ends up calling the same two methods:

```rust
pub unsafe trait GlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout);
}
```

`alloc` asks for memory; `dealloc` gives it back. The request is a **`Layout`** —
not just a byte count but a *size and an alignment*: "I need 24 bytes, aligned to
8." Alignment matters because a `u64` must sit on an 8-byte boundary or the CPU
faults (or, on x86, just runs slower); the allocator has to honor it. `alloc`
returns a raw `*mut u8` to a block that satisfies the layout, or **null** to say
"I couldn't." That's the entire contract, and it is the whole seam between Rust's
containers and us: implement those two methods and register the implementation,
and `Vec::push` starts working without any further cooperation from `Vec`.

Registering it is one attribute:

```rust
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::new();
```

`#[global_allocator]` tells the compiler "this is the thing `Vec` and friends
should call." Under the hood it generates the `__rust_alloc` / `__rust_dealloc`
symbols that the `alloc` crate's containers actually link against, forwarding them
to our trait methods. In `src/heap.rs` the registered allocator is `ALLOCATOR`, a
`LockedHeap`, and everything below is the story of what happens inside its `alloc`.

**Turning `Vec` on: the `alloc` crate.** In a `no_std` kernel the standard library
is gone, and with it `Vec`, `Box`, and `String` — they live in `std`. But they
don't *need* an OS; they only need an allocator. Rust factors exactly that subset
out into a separate crate, **`alloc`**, that sits between `core` (no allocation)
and `std` (full OS). You opt in at the crate root with one line:

```rust
extern crate alloc;
```

and now `use alloc::vec::Vec;` works. `alloc` supplies the containers; our
`#[global_allocator]` supplies the memory they run on. That's the deal.

**The honest surprise: the ceremony is tiny.** This milestone has a reputation for
being a swamp of nightly-only incantations, and for years it was. Pre-1.68 Rust
made you flip on `#![feature(...)]` gates and hand-write an
`#[alloc_error_handler]` — a whole separate function whose only job was to decide
what happens when the heap is empty. Tutorials from that era are thick with it. On
the pinned **stable** toolchain this kernel uses, essentially all of it is gone:

- **No `#![feature(...)]`.** `GlobalAlloc`, `Layout`, and the `alloc` crate are all
  stable now.
- **No `#[alloc_error_handler]`.** Since Rust 1.68 the default behavior on
  out-of-memory is to `panic!("memory allocation of N bytes failed")` — which
  routes straight through the `#[panic_handler]` the kernel already has. An OOM is
  just a panic; we wrote the panic handler milestones ago; nothing new is owed.

So the "ceremony" reduces to `extern crate alloc;`, the `#[global_allocator]`
static, and implementing two methods. The reputation is real but it belongs to the
past — worth knowing so you recognize the outdated advice when you meet it, and
worth *not* copying.

## Where the heap's bytes come from

The trait says *how* memory is requested. The next question is *what memory* the
allocator hands out — and this is where the last two milestones cash in.

The heap needs a home: a run of addresses it owns outright, big enough to carve up.
Ziran gives it a fixed slab of **virtual** address space:

```rust
pub const HEAP_BASE: u64 = 0x0000_0000_4000_0000; // 1 GiB
pub const HEAP_SIZE: u64 = 0x10_0000;             // 1 MiB = 256 pages
```

The base, `HEAP_BASE`, is exactly **1 GiB** — and that is not a round number
chosen for looks. It is the *first byte past the boot identity map*. Recall from
virtual-memory.md that the kernel identity-maps `[0, 1 GiB)` and that M7's
self-test deliberately picked `TEST_VIRT = 0x4000_0000`, one gigabyte, precisely
because nothing maps it "for free." The heap inherits that same honesty: a pointer
into the heap can resolve **only** through the kernel's own page tables, never
through the identity gift. If the mapping is wrong, it faults immediately rather
than accidentally working through a leftover boot mapping.

But a virtual range is just *names* — 1 MiB of addresses that translate to nothing
yet. To make it real memory, every page in it has to be backed by an actual
physical frame. That is the frames→pages machinery, doing exactly the job it was
built for. `heap::init` walks the region a page at a time:

```rust
pub fn init() {
    let mut virt = HEAP_BASE;
    let end = HEAP_BASE + HEAP_SIZE;
    while virt < end {
        let frame = frame_allocator::alloc().expect("heap: out of frames …");
        paging::map_page(virt, frame.start_address(), paging::WRITABLE)
            .expect("heap: map_page failed …");
        virt += PAGE_SIZE;
    }
    // … then hand [HEAP_BASE, HEAP_BASE + HEAP_SIZE) to the allocator …
}
```

Read that loop as the whole memory stack in four lines. `frame_allocator::alloc`
(M6) hands back a physical frame — a bit flipped in the bitmap from
physical-memory.md. `paging::map_page` (M7) installs a page-table entry so the
virtual address `virt` translates to that frame, writable. Do it 256 times and the
entire `[1 GiB, 1 GiB + 1 MiB)` window is backed by real RAM the CPU can touch.
**The heap literally sits on frames, under pages.**

Two deliberate choices hide in that loop:

- **Eager mapping.** We map *all* 256 pages up front, at init, before a single
  allocation. The alternative — *demand paging*, where a page is mapped lazily the
  first time it's touched and a page fault triggers the mapping — is genuinely
  useful, but it needs the M7 `#PF` handler to allocate-a-frame-and-retry the
  faulting instruction, which is real interrupt-return plumbing. That belongs to a
  later milestone. Eager mapping keeps this one about the *allocator*, not the
  fault path.
- **`map_page` can fail, and here we treat failure as fatal.** `map_page` returns a
  `Result` (it might run out of frames for an intermediate table). The heap's
  runtime `alloc` will lean on that fallibility. But `init` is one-time
  foundational setup, like `paging::init`: a kernel that can't even stand up its
  heap is unrecoverable, so a shortage here `panic`s rather than limping on.

A quiet payoff falls out of the address choice. Those 256 pages all land in
`[1 GiB, 1 GiB + 2 MiB)`, which shares a single PD and PT. M7's self-test already
mapped `0x4000_0000` and then unmapped it — and recall that `unmap_page`
deliberately leaves the intermediate tables behind (the "small, bounded leak"
virtual-memory.md was careful to make visible). So when `heap::init` runs, those
two tables are *already present*, and it allocates only the 256 heap frames. M7's
documented leak is quietly reused here. (If the tables weren't there, `map_page`
would build them; `init` works either way.)

## The free-list allocator: the core lesson

We have a mapped megabyte and a trait to implement. Now the actual question: given
one big block of memory, how do you hand out pieces of it — of varying sizes and
alignments — and, crucially, *take them back* so the space can be reused?

The naive answer is a **bump allocator**: keep a cursor at the start, and on every
`alloc` return the cursor and advance it by the request. One line, dead simple, and
*it is wrong for us for exactly the reason M6 rejected it for frames.* A bump
allocator has nowhere to record that a freed block is available again, so its
`dealloc` is a no-op and its cursor only ever marches forward. Feed it the
self-test's `Vec` that grows past capacity — which *reallocates*, allocating a
bigger buffer and freeing the old one — and every dropped intermediate buffer
leaks. It would make `Vec` *look* like it works while quietly hemorrhaging memory.
Milestone 6 held the frame allocator to "a real OS must reclaim"; the heap gets the
same standard.

So Ziran uses a **linked-list free-list**, and the elegant trick at its heart is
*where the list lives*. A free region is, by definition, memory nobody is using
right now — so we store the bookkeeping **inside the free region itself**. Each
free block begins with a small node:

```rust
struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}
```

`size` is how many bytes this free region spans; `next` points at the first bytes
of the *next* free region. The list threads through the free memory with **no side
table** — no separate array of "which blocks are free," because the free blocks
*are* the list. This is the **intrusive free-list** idea, and it is the very design
physical-memory.md said M6 *wanted* for frames but couldn't use: an intrusive list
needs to write a pointer into the free memory, and back then the only touchable RAM
was the identity map, with free frames potentially above it. M7 dissolved that —
now the heap's memory is mapped and writable, so the links have somewhere to live.
The idea that was a footgun for frames is exactly right for the heap.

Picture the list threaded through the heap after a few allocations and frees. `X`
is memory currently handed out (opaque to us); the free regions each carry a
`ListNode` in their first bytes:

```text
  head ─┐
        ▼
  ┌──────────────┐   ┌──────┐   ┌───────────────────────┐
  │ ListNode     │   │  X   │   │ ListNode              │  X  ...
  │ size=48      │   │ used │   │ size=512              │ used
  │ next ────────┼─┐ │      │ ┌▶│ next = None           │
  └──────────────┘ │ └──────┘ │ └───────────────────────┘
   free region     └──────────┘  free region
   (48 bytes)         skips       (512 bytes)
                      the used
                      block
```

The dummy `head` node (`FreeListHeap.head`) isn't a real region — it just holds the
`next` pointer to the first free block, so the list has a stable starting handle.
Every real free region is reachable by following `next` until it hits `None`.

Three operations run on this structure:

**Allocation is first-fit with a split.** `find_region` walks the list from `head`
and stops at the *first* region big enough to satisfy the request (`alloc_from_region`
does the check). "First that fits" — not the smallest, not the best — because it's
simple and adequate; refinements are a later problem. Once found, the region is
unlinked, the request is carved off the front, and the **remainder is pushed back
onto the list** as a smaller free region:

```text
   before:  ┌───────────── 512-byte free region ──────────────┐

   alloc(size=64):
            ┌── 64 handed out ──┐┌──── 448-byte remainder ─────┐
                                  └─ re-listed as a free region
```

That "return the tail" step (`add_free_region(alloc_end, excess)` in `alloc`) is
what keeps the heap from bleeding space on every allocation — the leftover goes
right back into circulation.

**Freeing pushes the block back.** `dealloc` does the one thing a bump allocator
can't: it threads a fresh `ListNode` through the returned block and pushes it onto
the front of the list. The bytes become reusable immediately. That is reclaim, and
it is the whole point.

```rust
unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
    let mut node = ListNode::new(size);
    node.next = self.head.next.take();      // old first-free becomes our next
    let node_ptr = addr as *mut ListNode;
    node_ptr.write(node);                    // write the node INTO the freed bytes
    self.head.next = Some(&mut *node_ptr);   // we are the new first-free
}
```

Note what `dealloc` *doesn't* do: it doesn't check for a double-free. The
`GlobalAlloc` contract forbids handing back the same block twice, and verifying it
would cost an O(n) list scan on every free — so the code trusts the contract and
says so in a comment. A violation would splice a block in twice and form a cycle,
not fail loudly; one more reason the contract is a contract.

**Alignment and the minimum block size.** Two constraints shape every request
before the list ever sees it, both handled by `size_align`:

- The returned pointer must be aligned as the `Layout` demands. `align_up` does the
  rounding, and it's worth seeing why the one-liner is correct:

  ```rust
  fn align_up(addr: usize, align: usize) -> usize {
      (addr + align - 1) & !(align - 1)
  }
  ```

  This is only valid because `align` is always a power of two (`Layout` guarantees
  it). Clearing the low bits (`& !(align - 1)`) rounds *down* to a multiple of
  `align`; biasing up by `align - 1` first turns that into rounding *up*. It's the
  same power-of-two-alignment identity the page-table index math leaned on in the
  sibling doc.
- Every allocation is rounded up to be **at least as large, and as aligned, as a
  `ListNode`**. Why: when the block is eventually freed, `dealloc` must write a
  `ListNode` into it (that's how it rejoins the list). A block too small or too
  ill-aligned to hold that node could never be reclaimed. `size_align` enforces the
  floor with `.align_to(align_of::<ListNode>())` and `.size().max(size_of::<ListNode>())`.

There's a matching guard on the split: if carving the request out of a region would
leave a remainder *smaller* than a `ListNode`, that remainder couldn't hold a node
and would be an untrackable sliver. `alloc_from_region` rejects such a fit and lets
first-fit continue to the next region, rather than create a fragment it can never
speak of again.

## The honest limitations

Following the same discipline the sibling docs insisted on — name what the design
*doesn't* do, out loud — this allocator has two known shortcomings. Neither
corrupts memory. Both merely waste it, and both are the natural next refinement:

- **First-fit with no coalescing.** When `dealloc` pushes a block back, it does
  *not* check whether the block sits physically adjacent to another free region and
  merge them. So a heap that allocates and frees in the wrong pattern can end up
  fragmented — plenty of total free space, but chopped into pieces too small for a
  large request that would fit if the neighbors were fused. A production allocator
  coalesces adjacent frees (and often keeps the list address-sorted to make that
  cheap). This one doesn't, on purpose: coalescing solves a performance problem we
  can't measure yet, exactly the "later refinement" hook that `unmap_page`'s
  deferred table reclaim was in M7.
- **The front-alignment-gap leak.** When a request needs more alignment than the
  region's start happens to provide, `align_up` skips forward a few bytes to reach
  an aligned start — and those skipped front-padding bytes are *not* re-listed as
  free. They leak. In this kernel it almost never bites: real alignments are ≤ 16,
  the heap base is 1 GiB-aligned, and blocks come out node-aligned, so the front gap
  is virtually always zero. But it's a real hole, and honesty means naming it.

Both are `waste`, not `corruption` — the distinction the whole project cares about.
Recovering the front gap and merging neighbors are precisely what a mature free
list adds on top of this one.

## Verification honesty: working vs. accidentally working

Physical-memory.md and virtual-memory.md were both careful about the gap between an
allocator that *works* and one that only *looks* like it works — and the heap has
the sharpest version of that trap yet, because a bump allocator would sail through
the obvious tests. `Box::new(5)` returning a usable pointer proves nothing about
reclaim; a `Vec` that sums correctly proves nothing about reclaim; even a `String`
that grows proves nothing about reclaim. A bump allocator passes *all of those* —
right up until it runs the machine out of memory.

So `heap::self_test` includes the one check a bump allocator provably cannot fake:
**allocate, record the address, free, allocate again — and demand the same address
back.**

```rust
let addr1 = { let x = Box::new(0u64); &*x as *const u64 as usize }; // x dropped here
let addr2 = { let y = Box::new(0u64); &*y as *const u64 as usize };
assert_eq!(addr1, addr2, "heap reclaim failed: a freed allocation was not reused");
```

The first `Box` is allocated and immediately dropped, freeing its block. The second
`Box`, of the same layout, must land at the **same address** — because a working
free list put the just-freed block back at the front of its list, so first-fit hands
it straight back. A bump allocator's cursor only advances; it would return a fresh,
higher address and fail this assert on the spot. This is the direct analogue of the
frame allocator's reclaim test from physical-memory.md ("the proof it's a real
bitmap and not a bump allocator"), lifted intact to the heap. Passing it is the
difference between a heap that reclaims and one that merely defers its leak.

The self-test earns its keep with a second, quieter assertion in the same spirit as
M7's "count the leaked tables." The heap manages a *pre-mapped* region, so runtime
`alloc`/`dealloc` should never need to touch the frame allocator — every byte it
hands out was mapped back at `init`. So the test records `frame_allocator::free_frame_count()`
before and after and asserts it is **unchanged**:

```rust
assert_eq!(frame_allocator::free_frame_count(), frames_before,
           "heap: runtime allocation should not touch the frame allocator");
```

If a heap allocation ever quietly consumed a physical frame, that would be an
accounting surprise — and, per the house rule the sibling docs kept repeating, an
accounting surprise is a bug. Make the expected number explicit and assert it.

## Where this goes next

The heap is the top stone of the memory stack, and unlike the two below it, its
customers aren't further plumbing — they're the *actual features* of the kernel.
Everything dynamic from here on is built out of the `Vec`, `Box`, and `String` this
milestone just switched on:

- **Milestone 9 (timer & scheduler)** needs a runtime list of tasks — a data
  structure that grows and shrinks as tasks come and go. That's a heap-backed
  collection.
- **Milestone 10 (the shell)** builds command lines and parses input into
  owned `String`s and `Vec`s sized to whatever the user types — impossible without
  a heap.
- **Milestone 11 (the filesystem)** holds directory entries, open-file tables, and
  buffers whose count and size are known only at runtime.

Each of those traces its dynamic memory back through `GlobalAlloc` to this
allocator, through `map_page` to a page, and through `frame_allocator::alloc` to a
bit in the M6 bitmap. That is the whole chain, now complete end to end.

That is the shape of the memory stack, finished: **frames (M6) → pages (M7) → heap
(M8).** This milestone lays the top stone — teaching the kernel, for the first
time, to hand out memory in amounts nobody knew until the moment it was asked.
