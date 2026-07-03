//! A minimal, read-only Multiboot2 boot-information parser — Milestone 6.
//!
//! When GRUB hands control to a Multiboot2 kernel it leaves, in memory, a single
//! "boot information" structure describing the machine: how much RAM there is and
//! where, the command line, the loader name, and more. Its physical address
//! arrives in `RDI` (see `kernel_main`). We only need one thing from it right
//! now — the **memory map** — so this module does the smallest honest job: walk
//! the structure and hand back the memory regions. It allocates nothing and owns
//! nothing; it borrows GRUB's structure in place and reads it.
//!
//! The layout (from the Multiboot2 spec, §3.6) is a header followed by a list of
//! variable-length, type-tagged records:
//!
//! ```text
//!   +0  total_size : u32     total bytes of the whole structure
//!   +4  reserved   : u32     always 0
//!   +8  first tag ...        a chain of { type:u32, size:u32, payload } records,
//!                            each padded up to the next 8-byte boundary, ending
//!                            with a terminator tag (type = 0, size = 8).
//! ```
//!
//! The memory map is the tag with `type = 6`. Its own little header gives an
//! `entry_size` — the authoritative stride between entries — followed by the
//! entries themselves, each `{ base_addr:u64, length:u64, kind:u32, reserved:u32 }`.
//! A `kind` of 1 means "usable RAM"; every other value is reserved/unusable.
//!
//! This module is a *dumb reader*: it reports raw regions and nothing more. All
//! policy about which frames are safe to hand out lives in `frame_allocator`.
//! See `docs/concepts/physical-memory.md` for the full explanation.

/// Multiboot2 tag type for the memory map (spec §3.6.8).
const TAG_TYPE_MEMORY_MAP: u32 = 6;
/// Multiboot2 tag type for the terminator that ends the tag list.
const TAG_TYPE_END: u32 = 0;
/// A memory-map entry's `kind` field: 1 means the region is usable RAM.
const MEMORY_KIND_USABLE: u32 = 1;

/// The common 8-byte header every Multiboot2 tag starts with.
#[repr(C)]
struct TagHeader {
    typ: u32,  // what kind of tag this is
    size: u32, // total bytes of this tag, including this header, before padding
}

/// One entry of the memory-map tag: a single contiguous physical region.
#[repr(C)]
struct RawMemoryEntry {
    base_addr: u64, // physical start address
    length: u64,    // size in bytes
    kind: u32,      // 1 = usable RAM; other values reserved/unusable
    _reserved: u32, // spec-mandated padding, always 0
}

/// A borrowed handle onto GRUB's boot-information structure.
///
/// Holds only the base address and the total size read from its header — the
/// data itself stays where GRUB left it. Construct with [`BootInfo::new`].
pub struct BootInfo {
    addr: u64,
    total_size: u32,
}

impl BootInfo {
    /// Wrap the Multiboot2 boot-information structure at physical `addr`.
    ///
    /// SAFETY: `addr` must be the address of a valid Multiboot2 boot-information
    /// structure (the value GRUB placed in `RDI`), and that memory must be
    /// mapped and readable. The boot page tables identity-map the first 1 GiB and
    /// GRUB places the structure in low memory, so the pointer GRUB hands us
    /// satisfies this. We read `total_size` from its first dword so later tag
    /// walks can be bounded by it.
    pub unsafe fn new(addr: u64) -> BootInfo {
        // SAFETY: guaranteed by this function's own contract: `addr` points at a
        // valid, mapped Multiboot2 structure, whose first field is `total_size`.
        let total_size = unsafe { core::ptr::read_unaligned(addr as *const u32) };
        BootInfo { addr, total_size }
    }

    /// The total byte span of the structure, `[addr, addr + total_size)`. The
    /// frame allocator reserves this range so it never hands out the frames the
    /// boot information itself occupies.
    pub fn region(&self) -> (u64, u64) {
        (self.addr, self.addr.saturating_add(self.total_size as u64))
    }

    /// Iterate the physical memory regions, or `None` if GRUB provided no memory
    /// map tag at all. The caller decides what a missing map means (the frame
    /// allocator treats it as fatal).
    pub fn memory_map(&self) -> Option<MemoryMapIter> {
        // Tags begin at +8: past `total_size` (u32) and `reserved` (u32).
        let mut tag_ptr = self.addr + 8;
        let end = self.addr + self.total_size as u64;

        while tag_ptr + core::mem::size_of::<TagHeader>() as u64 <= end {
            // SAFETY: `tag_ptr` stays within [addr, addr + total_size) by the
            // loop guard, and that whole span is the mapped structure promised to
            // `BootInfo::new`. Reads are unaligned-safe.
            let header: TagHeader =
                unsafe { core::ptr::read_unaligned(tag_ptr as *const TagHeader) };

            // A valid tag is at least its own 8-byte header. A smaller size is
            // malformed and, worse, would make the `(size + 7) & !7` advance
            // below round to 0 and spin this loop forever — stop the walk.
            if (header.size as usize) < core::mem::size_of::<TagHeader>() {
                return None;
            }

            if header.typ == TAG_TYPE_END {
                return None;
            }

            if header.typ == TAG_TYPE_MEMORY_MAP {
                // Memory-map tag payload: entry_size (u32) and entry_version (u32)
                // precede the entries, so the first entry is at tag_ptr + 16. We
                // stride by the tag's own `entry_size`, never by size_of, so a
                // loader that pads entries wider than 24 bytes can't desync us.
                // SAFETY: same mapped-structure guarantee; entry_size lives at
                // tag_ptr + 8, inside this tag.
                let entry_size =
                    unsafe { core::ptr::read_unaligned((tag_ptr + 8) as *const u32) } as u64;
                // An entry can't be smaller than one entry record; a zero (or
                // too-small) stride would make the iterator loop without ever
                // advancing and over-read each entry. Reject rather than hang.
                if entry_size < core::mem::size_of::<RawMemoryEntry>() as u64 {
                    return None;
                }
                return Some(MemoryMapIter {
                    next: tag_ptr + 16,
                    // Clamp the tag's self-reported end to the structure's own
                    // bound: a corrupt oversized `size` must not let the iterator
                    // march past mapped memory (below the IDT exists) into a fault.
                    end: (tag_ptr + header.size as u64).min(end),
                    entry_size,
                });
            }

            // Advance to the next tag, rounding this tag's size up to the next
            // 8-byte boundary (tags are 8-byte aligned; `size` excludes padding).
            tag_ptr += (header.size as u64 + 7) & !7;
        }

        None
    }
}

/// Iterator over the memory-map entries. Yields [`MemoryRegion`]s in the order
/// GRUB listed them.
pub struct MemoryMapIter {
    next: u64,       // address of the next entry to read
    end: u64,        // one past the last byte of the memory-map tag
    entry_size: u64, // authoritative stride between entries
}

impl Iterator for MemoryMapIter {
    type Item = MemoryRegion;

    fn next(&mut self) -> Option<MemoryRegion> {
        if self.next + self.entry_size > self.end {
            return None;
        }
        // SAFETY: `next` points at a full entry inside the memory-map tag (the
        // guard above proves `[next, next + entry_size)` is within the tag), and
        // the tag lies inside the mapped structure promised to `BootInfo::new`.
        let raw: RawMemoryEntry =
            unsafe { core::ptr::read_unaligned(self.next as *const RawMemoryEntry) };
        self.next += self.entry_size;
        Some(MemoryRegion {
            base: raw.base_addr,
            length: raw.length,
            kind: raw.kind,
        })
    }
}

/// One contiguous physical memory region, as reported by GRUB.
#[derive(Clone, Copy)]
pub struct MemoryRegion {
    /// Physical start address.
    pub base: u64,
    /// Length in bytes.
    pub length: u64,
    /// Region kind; 1 is usable RAM, everything else reserved.
    pub kind: u32,
}

impl MemoryRegion {
    /// One past the last byte of the region. Saturating, so a garbage `length`
    /// from a malformed map clamps to `u64::MAX` instead of wrapping to a small
    /// value that would look like a tiny (or inverted) region.
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.length)
    }

    /// Whether this region is usable RAM (`kind == 1`) rather than reserved.
    pub fn is_usable(&self) -> bool {
        self.kind == MEMORY_KIND_USABLE
    }
}
