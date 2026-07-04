//! The kernel heap and global allocator — Milestone 8.
//!
//! This is the milestone where `alloc` — `Vec`, `Box`, `String`, all of Rust's
//! dynamic memory — starts working with no operating system underneath. Every one
//! of those types bottoms out in a single trait, `GlobalAlloc`, that the kernel
//! implements; and the memory it hands out lives in a region of *virtual* memory
//! the kernel mapped for itself, backed by physical frames from Milestone 6.
//!
//! So the heap sits exactly on top of the last two milestones: it reserves a
//! virtual range, asks Milestone 7's `map_page` to back each of its pages with a
//! Milestone 6 frame, then manages the resulting bytes. That completes the stack
//! the memory work has been building toward: **frames (M6) → pages (M7) → heap
//! (M8)**. See `docs/concepts/heap.md`.
//!
//! The allocator is a **linked-list free-list**: every free region stores a small
//! `ListNode { size, next }` *inside its own memory*, so the free list threads
//! through the free bytes themselves with no side table. This is the intrusive
//! free-list idea Milestone 6 wanted but couldn't use for frames (there was no
//! mapped memory to store the links in yet); Milestone 7 fixed that. A bump
//! allocator was rejected for the same reason M6 rejected one — it cannot free an
//! individual allocation, so it leaks every `Vec` reallocation. This one reclaims.
//!
//! Two honest limitations, both later-refinement hooks — neither corrupts memory,
//! both merely waste it: (1) `dealloc` does **not** coalesce adjacent free regions,
//! so fragmentation can accumulate; and (2) an over-aligned request (align > 16)
//! leaks the small front-padding gap it skips to reach an aligned start — those
//! bytes are not re-listed. In this kernel real alignments are ≤ 16 off a
//! 1 GiB-aligned heap base, so the front gap is almost always zero. Merging
//! neighbours and recovering the front gap are what a production free list adds.

use crate::{frame_allocator, paging};
use core::alloc::{GlobalAlloc, Layout};
use core::mem::{align_of, size_of};
use spin::Mutex;

/// Base of the kernel heap: exactly 1 GiB, the first byte past the boot identity
/// map. A pointer here can only resolve through the kernel's own page tables —
/// never the identity gift — which keeps the milestone honest. It is also the
/// address M7's self-test already proved a frame maps and round-trips at.
pub const HEAP_BASE: u64 = 0x0000_0000_4000_0000;
/// Size of the kernel heap: 1 MiB = 256 pages. ~0.8% of usable RAM; ample for
/// `Vec`/`Box`/`String` in the self-test and the milestones that follow.
pub const HEAP_SIZE: u64 = 0x10_0000;
/// 4 KiB page size.
const PAGE_SIZE: u64 = 4096;

/// Round `addr` up to a multiple of `align`. Valid only because `align` is a
/// power of two (`Layout` guarantees it): clearing the low bits rounds down, so
/// biasing by `align - 1` first rounds up. The `addr + align - 1` add is
/// unchecked, which is sound only because our heap addresses are ~1 GiB and
/// `align` is a `Layout` power of two — the sum cannot wrap `usize`.
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// A node in the free list, stored at the start of the free region it describes.
struct ListNode {
    size: usize,
    next: Option<&'static mut ListNode>,
}

impl ListNode {
    const fn new(size: usize) -> ListNode {
        ListNode { size, next: None }
    }
    fn start_addr(&self) -> usize {
        self as *const ListNode as usize
    }
    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

/// The free-list allocator. `head` is a dummy node; `head.next` points at the
/// first real free region.
struct FreeListHeap {
    head: ListNode,
}

impl FreeListHeap {
    const fn new() -> FreeListHeap {
        FreeListHeap {
            head: ListNode::new(0),
        }
    }

    /// Adopt the backed region `[start, start + size)` as one initial free block.
    ///
    /// SAFETY: the region must be mapped, writable, and owned by this allocator
    /// alone; `start` must be `align_of::<ListNode>()`-aligned and `size` at least
    /// `size_of::<ListNode>()`.
    unsafe fn init(&mut self, start: usize, size: usize) {
        unsafe { self.add_free_region(start, size) };
    }

    /// Push the free region `[addr, addr + size)` onto the front of the list,
    /// writing its bookkeeping node into the region's own first bytes.
    ///
    /// SAFETY: `[addr, addr + size)` must be mapped, writable, and not otherwise
    /// in use; it must be node-aligned and node-sized (guaranteed by `size_align`
    /// for allocations, and by the caller for `init`).
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        debug_assert_eq!(align_up(addr, align_of::<ListNode>()), addr);
        assert!(size >= size_of::<ListNode>(), "heap: free region smaller than a list node");
        let mut node = ListNode::new(size);
        node.next = self.head.next.take();
        let node_ptr = addr as *mut ListNode;
        // SAFETY: `addr` is a node-aligned, node-sized region we own.
        unsafe {
            node_ptr.write(node);
            self.head.next = Some(&mut *node_ptr);
        }
    }

    /// First-fit: find and unlink the first region that can satisfy `size`/`align`,
    /// returning the removed region and the aligned allocation start.
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut ListNode, usize)> {
        let mut current = &mut self.head;
        while let Some(ref mut region) = current.next {
            if let Ok(alloc_start) = Self::alloc_from_region(region, size, align) {
                let next = region.next.take();
                let ret = Some((current.next.take().unwrap(), alloc_start));
                current.next = next;
                return ret;
            }
            current = current.next.as_mut().unwrap();
        }
        None
    }

    /// Can `region` hold `size` bytes at `align`? Returns the aligned start. The
    /// remainder after the allocation must be either zero or big enough to hold a
    /// `ListNode` — otherwise it would be an untrackable sliver, so reject and let
    /// the caller try the next region.
    fn alloc_from_region(region: &ListNode, size: usize, align: usize) -> Result<usize, ()> {
        let alloc_start = align_up(region.start_addr(), align);
        let alloc_end = alloc_start.checked_add(size).ok_or(())?;
        if alloc_end > region.end_addr() {
            return Err(()); // doesn't fit
        }
        let excess = region.end_addr() - alloc_end;
        if excess > 0 && excess < size_of::<ListNode>() {
            return Err(()); // leftover too small to track
        }
        Ok(alloc_start)
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let (size, align) = size_align(layout);
        if let Some((region, alloc_start)) = self.find_region(size, align) {
            let alloc_end = alloc_start + size; // checked in alloc_from_region
            let excess = region.end_addr() - alloc_end;
            if excess > 0 {
                // Return the tail of the region to the free list.
                // SAFETY: `[alloc_end, region.end)` is within the region we just
                // unlinked; it is node-sized (checked) and mapped/writable.
                unsafe { self.add_free_region(alloc_end, excess) };
            }
            alloc_start as *mut u8
        } else {
            core::ptr::null_mut() // honest OOM — not a fake pointer
        }
    }

    fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _) = size_align(layout);
        // Reclaim: thread a fresh node through the returned block. This is what a
        // bump allocator cannot do — the freed bytes become reusable. We do NOT
        // detect a double free: the GlobalAlloc contract forbids it, and checking
        // would cost an O(n) list scan on every free. A violation would splice the
        // block into the list twice, forming a cycle rather than failing loudly —
        // one more reason the contract must be trusted.
        // SAFETY: `ptr`/`layout` came from this allocator (caller's contract), so
        // `[ptr, ptr+size)` is a node-sized, node-aligned block we own.
        unsafe { self.add_free_region(ptr as usize, size) };
    }
}

/// Round a request up so every allocation is at least as large and as aligned as a
/// `ListNode` — a freed block must be able to hold the node that re-links it.
fn size_align(layout: Layout) -> (usize, usize) {
    let layout = layout
        .align_to(align_of::<ListNode>())
        .expect("heap: requested alignment is unreasonably large")
        .pad_to_align();
    let size = layout.size().max(size_of::<ListNode>());
    (size, layout.align())
}

/// Newtype wrapping the allocator in a `spin::Mutex`. `GlobalAlloc`'s methods take
/// `&self`, but the allocator must mutate; the Mutex supplies interior mutability,
/// and the newtype exists because the orphan rule forbids `impl GlobalAlloc for
/// Mutex<…>` directly. Uncontended today (single core, never entered from an
/// interrupt handler) — the M9 revisit, like the frame allocator and paging.
pub struct LockedHeap(Mutex<FreeListHeap>);

impl LockedHeap {
    const fn new() -> LockedHeap {
        LockedHeap(Mutex::new(FreeListHeap::new()))
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    // SAFETY: returns a `layout.align()`-aligned pointer to `layout.size()` bytes
    // inside the mapped heap, or null on failure — the GlobalAlloc contract.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.lock().alloc(layout)
    }

    // SAFETY: `ptr`/`layout` came from this allocator's `alloc` (caller's contract).
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.0.lock().dealloc(ptr, layout)
    }
}

/// The one global allocator. `#[global_allocator]` wires Rust's `Vec`/`Box`/… to
/// it and generates the `__rust_alloc` symbols the staticlib exports.
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::new();

/// Reserve the heap's virtual region, eagerly map every page onto a fresh frame,
/// and hand the region to the allocator. Call once from `kernel_main`, after
/// `paging::init()` (it maps through the kernel's own tables) and before any
/// allocation. Panics on frame exhaustion — a heap the kernel can't stand up this
/// early is unrecoverable, like `paging::init`.
pub fn init() {
    let mut virt = HEAP_BASE;
    let end = HEAP_BASE + HEAP_SIZE;
    while virt < end {
        let frame = frame_allocator::alloc().expect("heap: out of frames mapping the kernel heap");
        paging::map_page(virt, frame.start_address(), paging::WRITABLE)
            .expect("heap: map_page failed building the kernel heap");
        virt += PAGE_SIZE;
    }

    // SAFETY: every page of [HEAP_BASE, HEAP_BASE + HEAP_SIZE) was just mapped
    // writable onto a distinct frame above the identity map; nothing else owns it,
    // and HEAP_BASE (1 GiB) is node-aligned.
    unsafe {
        ALLOCATOR.0.lock().init(HEAP_BASE as usize, HEAP_SIZE as usize);
    }

    crate::serial_println!(
        "[ok] heap: {} KiB mapped at {:#014x} ({} pages)",
        HEAP_SIZE / 1024,
        HEAP_BASE,
        HEAP_SIZE / PAGE_SIZE
    );
    crate::serial_println!("M8: kernel heap online");
}

/// Prove the allocator works over serial: `Box`, a `Vec` grown past its capacity,
/// a `String`, a multi-page allocation, and — the honesty check — that a freed
/// allocation's memory is reused, the one thing a bump allocator provably cannot
/// do. The heap allocates within its pre-mapped region, so it must not touch the
/// frame allocator at runtime; we assert that too.
pub fn self_test() {
    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::vec::Vec;

    let frames_before = frame_allocator::free_frame_count();

    // Box, Vec (grown past capacity -> real reallocation), String.
    let boxed = Box::new(0x00C0_FFEE_u64);
    assert_eq!(*boxed, 0x00C0_FFEE, "heap: Box lost its value");

    let mut v = Vec::new();
    for i in 0..256u64 {
        v.push(i);
    }
    assert_eq!(v.iter().sum::<u64>(), (0..256u64).sum(), "heap: Vec lost data");

    let mut s = String::from("Ziran");
    s.push_str(" OS heap");
    assert_eq!(s, "Ziran OS heap", "heap: String corrupted");

    // A single allocation spanning several pages, to prove the whole region is
    // mapped, not just the first page.
    let big: Vec<u64> = (0..2048u64).collect(); // 16 KiB across 4 pages
    assert_eq!(big[2047], 2047, "heap: multi-page allocation not fully mapped");

    crate::serial_println!("[ok] heap: Box, Vec (grown), String, and a 4-page Vec all allocate");

    drop((boxed, v, s, big));

    // The honesty check: allocate, free, allocate again -> the SAME address comes
    // back. A bump allocator's cursor only advances; it would return a fresh,
    // higher address and fail this. This is the reclaim proof, lifted from the
    // frame allocator's self-test to the heap.
    let addr1 = {
        let x = Box::new(0u64);
        &*x as *const u64 as usize
    }; // x dropped here -> region freed
    let addr2 = {
        let y = Box::new(0u64);
        &*y as *const u64 as usize
    };
    assert_eq!(
        addr1, addr2,
        "heap reclaim failed: a freed allocation was not reused by the next alloc"
    );
    crate::serial_println!(
        "[ok] heap: freed {:#014x} and got the same address back (reclaim works)",
        addr2
    );

    // The heap manages its pre-mapped region; runtime alloc/free must not consume
    // physical frames.
    assert_eq!(
        frame_allocator::free_frame_count(),
        frames_before,
        "heap: runtime allocation should not touch the frame allocator"
    );
}
