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
//! (Build step 1: a throwaway *bump* allocator, here only to prove the `alloc`
//! ceremony links and the region maps. Step 2 replaces it with a free-list that
//! can actually reclaim — a bump allocator leaks every `Vec` reallocation, the
//! same reason Milestone 6 rejected one for the frame allocator.)

use crate::{frame_allocator, paging};
use core::alloc::{GlobalAlloc, Layout};
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
/// biasing by `align - 1` first rounds up.
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// A bump allocator: hand out memory by advancing a cursor. Trivial, and enough
/// to prove the plumbing — but it cannot free an individual allocation; only when
/// *every* allocation is released does the cursor reset. Step 2 replaces it.
struct BumpHeap {
    start: usize,
    next: usize,
    end: usize,
    allocations: usize,
}

impl BumpHeap {
    const fn new() -> BumpHeap {
        BumpHeap {
            start: 0,
            next: 0,
            end: 0,
            allocations: 0,
        }
    }

    /// Adopt the backed region `[start, start + size)`.
    ///
    /// SAFETY: the caller guarantees the region is mapped, writable, and owned by
    /// this allocator alone.
    unsafe fn init(&mut self, start: usize, size: usize) {
        self.start = start;
        self.next = start;
        self.end = start + size;
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let start = align_up(self.next, layout.align());
        let end = match start.checked_add(layout.size()) {
            Some(end) => end,
            None => return core::ptr::null_mut(),
        };
        if end > self.end {
            core::ptr::null_mut() // heap exhausted — honest null, not a fake pointer
        } else {
            self.next = end;
            self.allocations += 1;
            start as *mut u8
        }
    }

    fn dealloc(&mut self) {
        // A bump allocator can only reclaim when the last live allocation goes.
        self.allocations -= 1;
        if self.allocations == 0 {
            self.next = self.start;
        }
    }
}

/// Newtype wrapping the allocator in a `spin::Mutex`. `GlobalAlloc`'s methods take
/// `&self`, but the allocator must mutate; the Mutex supplies interior mutability,
/// and the newtype exists because the orphan rule forbids `impl GlobalAlloc for
/// Mutex<…>` directly. Uncontended today (single core, never entered from an
/// interrupt handler) — the M9 revisit, like the frame allocator and paging.
pub struct LockedHeap(Mutex<BumpHeap>);

impl LockedHeap {
    const fn new() -> LockedHeap {
        LockedHeap(Mutex::new(BumpHeap::new()))
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    // SAFETY: the returned pointer is `layout.align()`-aligned and points at
    // `layout.size()` bytes inside the mapped heap region, or is null on failure —
    // exactly the GlobalAlloc contract.
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.lock().alloc(layout)
    }

    // SAFETY: `ptr`/`layout` came from this allocator's `alloc` (the caller's
    // contract); the bump allocator only counts down toward a full reset.
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        self.0.lock().dealloc();
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
    // writable onto a distinct frame above the identity map; nothing else owns it.
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

/// Prove `alloc` works: a `Box`, and a `Vec` pushed past its initial capacity so
/// it must reallocate *through* the allocator. (Step 2 adds the reclaim check.)
pub fn self_test() {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    let boxed = Box::new(0x00C0_FFEE_u64);
    assert_eq!(*boxed, 0x00C0_FFEE, "heap: Box did not hold its value");

    let mut v = Vec::new();
    for i in 0..256u64 {
        v.push(i);
    }
    assert_eq!(
        v.iter().sum::<u64>(),
        (0..256u64).sum(),
        "heap: Vec lost data across its reallocations"
    );

    crate::serial_println!("[ok] heap: Box and a Vec grown past capacity both allocate");
}
