//! A bitmap physical-frame allocator — Milestone 6.
//!
//! Physical memory is handed out in fixed 4 KiB units called *frames* (the page
//! size the x86-64 MMU works in). This module owns the one authoritative answer
//! to "which physical frames are free?" — every later subsystem that needs real
//! memory (page tables in M7, the heap in M8) gets its frames from here.
//!
//! The design is a **bitmap**: one bit per 4 KiB frame, `0 = free`, `1 = used`.
//! `alloc` scans for the first `0` bit and flips it; `free` clears a bit. We use
//! a bitmap rather than a bump/"next-free" pointer precisely because a real OS
//! must *reclaim* memory, and a bump allocator cannot free. An intrusive
//! free-list was rejected too: it would store its next-pointer *inside* each free
//! frame, which we cannot safely touch for frames above the 1 GiB identity map
//! until paging exists (M7). A bitmap keeps all its state in one place — a static
//! array in `.bss` — and needs no memory to bootstrap itself.
//!
//! The bitmap is sized for a fixed **4 GiB** of coverage: 4 GiB / 4 KiB = 1 Mi
//! frames = 1 Mi bits = 128 KiB. That is a compile-time `.bss` array, so there
//! is no chicken-and-egg problem of "which frame holds the allocator's own data"
//! — it lives inside the kernel image, which `init` reserves along with the rest.
//!
//! Policy lives here; parsing does not. [`crate::multiboot`] is a dumb reader
//! that yields raw regions; this module decides which frames are safe to give
//! out. See `docs/concepts/physical-memory.md`.

use crate::multiboot::BootInfo;
use spin::Mutex;

/// The size of a physical frame, in bytes. 4 KiB is the x86-64 base page size.
const FRAME_SIZE: u64 = 4096;
/// Highest physical address the bitmap tracks: 4 GiB. RAM above this is reported
/// but not managed (see the ignored-RAM note in `init`).
const MAX_ADDRESS: u64 = 4 * 1024 * 1024 * 1024;
/// Number of frames the bitmap covers: 4 GiB / 4 KiB = 1,048,576.
const FRAME_COUNT: usize = (MAX_ADDRESS / FRAME_SIZE) as usize;
/// Bitmap storage, as 64-bit words: 1,048,576 bits / 64 = 16,384 words = 128 KiB.
const BITMAP_WORDS: usize = FRAME_COUNT / 64;

/// A single 4 KiB physical frame, identified by its (frame-aligned) start
/// address. This is the currency `alloc`/`free` trade in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame(u64);

impl PhysFrame {
    /// The frame that contains `addr`, rounding down to the 4 KiB boundary.
    // Part of the intended `PhysFrame` API; the page-table code in M7 is the
    // first caller. Kept now so the type is complete where it is defined.
    #[allow(dead_code)]
    pub fn containing_address(addr: u64) -> PhysFrame {
        PhysFrame(addr & !(FRAME_SIZE - 1))
    }

    /// The frame's physical start address (always 4 KiB-aligned).
    pub fn start_address(&self) -> u64 {
        self.0
    }

    /// The frame's index into the bitmap: `address / 4096`.
    fn index(&self) -> usize {
        (self.0 / FRAME_SIZE) as usize
    }
}

/// The allocator itself: the bitmap plus a running count of frames it considers
/// free, kept only so `init` can report a summary.
pub struct FrameAllocator {
    // One bit per frame, `1 = used`. Starts all-zero (all "free") at compile time
    // so it lives in `.bss`; `init` immediately marks everything used and then
    // frees only what GRUB reports as usable RAM — "never hand out the unknown."
    bitmap: [u64; BITMAP_WORDS],
    free_frames: u64,
}

impl FrameAllocator {
    /// A fresh allocator with an all-zero bitmap. Not usable until [`init`] has
    /// populated it from the memory map.
    const fn new() -> FrameAllocator {
        FrameAllocator {
            bitmap: [0; BITMAP_WORDS],
            free_frames: 0,
        }
    }

    /// Mark the single frame at `index` as used.
    fn set_used(&mut self, index: usize) {
        self.bitmap[index / 64] |= 1 << (index % 64);
    }

    /// Mark the single frame at `index` as free.
    fn set_free(&mut self, index: usize) {
        self.bitmap[index / 64] &= !(1 << (index % 64));
    }

    /// Whether the frame at `index` is currently marked used.
    fn is_used(&self, index: usize) -> bool {
        self.bitmap[index / 64] & (1 << (index % 64)) != 0
    }

    /// Reserve every frame that overlaps `[start, end)` — rounding the start down
    /// and the end up so a partially-covered frame is treated as fully used. This
    /// is the conservative direction: it can only ever mark *more* memory used.
    fn reserve(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        let first = (start / FRAME_SIZE) as usize;
        let last = ((end + FRAME_SIZE - 1) / FRAME_SIZE) as usize; // exclusive
        for index in first..last.min(FRAME_COUNT) {
            if !self.is_used(index) {
                self.free_frames -= 1;
            }
            self.set_used(index);
        }
    }
}

/// The one global frame allocator. A spin `Mutex` matches the house style for
/// mutable singletons; it is uncontended today (single core, and never touched
/// from an interrupt handler yet — that guarantee is revisited at M9).
static FRAME_ALLOCATOR: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::new());

/// Build the physical memory map from GRUB's boot information and stand the
/// allocator up. Must be called exactly once, early in `kernel_main`.
///
/// `multiboot_info_addr` is GRUB's Multiboot2 pointer; `kernel_start`/`kernel_end`
/// are the linker symbols bounding the loaded kernel image. Panics if GRUB
/// supplied no memory map — proceeding with an empty one would silently hand out
/// nonexistent RAM.
///
/// SAFETY: `multiboot_info_addr` must be the valid Multiboot2 pointer from the
/// boot handoff, pointing at mapped memory (it is, per `kernel_main`).
pub unsafe fn init(multiboot_info_addr: u64, kernel_start: u64, kernel_end: u64) {
    // SAFETY: forwarded from this function's own contract.
    let boot_info = unsafe { BootInfo::new(multiboot_info_addr) };
    let memory_map = boot_info
        .memory_map()
        .expect("Multiboot2 info has no memory-map tag — cannot manage physical memory");

    let mut allocator = FRAME_ALLOCATOR.lock();

    // Phase 1: mark the entire address space used. Anything GRUB does not
    // explicitly call usable stays used, so a gap in the map is never handed out.
    for word in allocator.bitmap.iter_mut() {
        *word = u64::MAX;
    }

    // Phase 2: free the frames fully contained in each usable region. Round the
    // base UP and the end DOWN, so a frame straddling a usable/reserved boundary
    // is left reserved. Clamp to the bitmap's 4 GiB span and tally any RAM above
    // it, which we report rather than silently drop (house rule: no silent caps).
    let mut usable_regions = 0u64;
    let mut ignored_bytes = 0u64;
    for region in memory_map {
        if !region.is_usable() {
            continue;
        }
        if region.base >= MAX_ADDRESS {
            ignored_bytes += region.length;
            continue;
        }
        let region_end = region.end().min(MAX_ADDRESS);
        if region.end() > MAX_ADDRESS {
            ignored_bytes += region.end() - MAX_ADDRESS;
        }
        let first = ((region.base + FRAME_SIZE - 1) / FRAME_SIZE) as usize; // round up
        let last = (region_end / FRAME_SIZE) as usize; // round down, exclusive
        if last <= first {
            continue; // region too small to contain a whole frame
        }
        usable_regions += 1;
        for index in first..last {
            if allocator.is_used(index) {
                allocator.free_frames += 1;
                allocator.set_free(index);
            }
        }
    }

    let usable_frames = allocator.free_frames;

    // Phase 3: re-reserve the frames we must never give away, even though they
    // sit inside usable RAM:
    //   - frame 0, so a physical address of 0 can always mean "null";
    //   - the loaded kernel image, which includes the boot page tables and stack
    //     (they live in .bss, inside [kernel_start, kernel_end));
    //   - the Multiboot2 structure itself, which GRUB placed in usable memory.
    allocator.reserve(0, FRAME_SIZE);
    allocator.reserve(kernel_start, kernel_end);
    let (mb_start, mb_end) = boot_info.region();
    allocator.reserve(mb_start, mb_end);

    let free_frames = allocator.free_frames;
    drop(allocator);

    // Report: the usable total from the map, then what remains after carve-outs.
    crate::serial_println!(
        "[ok] physical memory: {} MiB usable across {} regions ({} frames)",
        usable_frames / 256, // frames * 4 KiB / 1 MiB == frames / 256
        usable_regions,
        usable_frames
    );
    crate::serial_println!(
        "[ok] frame allocator: {} frames free after reserving kernel + boot info",
        free_frames
    );
    if ignored_bytes != 0 {
        crate::serial_println!(
            "[warn] {} MiB of RAM above the {} GiB bitmap limit is unmanaged",
            ignored_bytes / (1024 * 1024),
            MAX_ADDRESS / (1024 * 1024 * 1024)
        );
    }
    // The stable marker CI greps for to know this milestone still works.
    crate::serial_println!("M6: frame allocator online");
}

/// Hand out a free physical frame, or `None` if none remain. The caller owns the
/// frame until it passes it back to [`free`]; it must never `unwrap()` this
/// blindly, since exhaustion is a real, recoverable outcome.
pub fn alloc() -> Option<PhysFrame> {
    let mut allocator = FRAME_ALLOCATOR.lock();
    for word_index in 0..BITMAP_WORDS {
        let word = allocator.bitmap[word_index];
        if word != u64::MAX {
            let bit = word.trailing_ones() as usize; // first 0 bit == first free frame
            let index = word_index * 64 + bit;
            allocator.set_used(index);
            allocator.free_frames -= 1;
            return Some(PhysFrame((index as u64) * FRAME_SIZE));
        }
    }
    None
}

/// Return a frame to the free pool. A double free (freeing a frame that is
/// already free) is reported rather than silently corrupting the free count —
/// clearing an already-clear bit would under-count and eventually hand the same
/// frame out twice.
pub fn free(frame: PhysFrame) {
    let index = frame.index();
    if index >= FRAME_COUNT {
        crate::serial_println!(
            "[warn] frame_allocator: free of out-of-range frame {:#018x}",
            frame.start_address()
        );
        return;
    }
    let mut allocator = FRAME_ALLOCATOR.lock();
    if !allocator.is_used(index) {
        crate::serial_println!(
            "[warn] frame_allocator: double free of frame {:#018x}",
            frame.start_address()
        );
        return;
    }
    allocator.set_free(index);
    allocator.free_frames += 1;
}

/// A self-test proving the allocator's core invariants, printed over serial so a
/// boot run demonstrates the milestone. It allocates a handful of frames, checks
/// they are distinct and frame-aligned, then proves *reclaim* works — the one
/// behaviour that separates a real bitmap from a bump allocator: free a frame and
/// the very next `alloc` hands it straight back.
pub fn self_test() {
    // Grab a few frames and sanity-check them.
    let a = alloc().expect("frame allocator empty at self-test");
    let b = alloc().expect("frame allocator empty at self-test");
    let c = alloc().expect("frame allocator empty at self-test");
    assert!(a != b && b != c && a != c, "allocated frames must be distinct");
    assert!(
        a.start_address() % FRAME_SIZE == 0
            && b.start_address() % FRAME_SIZE == 0
            && c.start_address() % FRAME_SIZE == 0,
        "allocated frames must be 4 KiB-aligned"
    );
    crate::serial_println!(
        "[ok] alloc: handed out {:#014x}, {:#014x}, {:#014x}",
        a.start_address(),
        b.start_address(),
        c.start_address()
    );

    // Reclaim test: free b, then the next alloc must return b.
    free(b);
    let reclaimed = alloc().expect("frame allocator empty after free");
    assert!(
        reclaimed == b,
        "reclaim failed: freed a frame but alloc did not return it"
    );
    crate::serial_println!(
        "[ok] reclaim: freed {:#014x} and got it back on the next alloc",
        reclaimed.start_address()
    );

    // Put the frames we borrowed for the test back.
    free(a);
    free(reclaimed);
    free(c);
}
