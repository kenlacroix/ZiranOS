//! 4-level paging / virtual memory — Milestone 7.
//!
//! The CPU does not address physical RAM directly once paging is on. Every memory
//! access uses a *virtual* address, and the MMU translates it to a *physical* one
//! by walking a tree of page tables the kernel builds. This is the illusion every
//! later abstraction — the heap, and eventually separate address spaces — is built
//! on: the kernel decides what virtual address maps to what physical frame.
//!
//! On x86-64 the tree is four levels deep. A 48-bit virtual address is four 9-bit
//! indices plus a 12-bit page offset:
//!
//! ```text
//!   47      39 38      30 29      21 20      12 11        0
//!   |  PML4   |  PDPT    |   PD     |   PT     |  offset   |
//!      (512-entry tables; each entry is 8 bytes -> one table = one 4 KiB frame)
//! ```
//!
//! CR3 points at the PML4. Each entry holds the physical frame of the next table
//! (or, with the PS bit, maps a large page directly — the boot code uses 2 MiB
//! pages). The walk reads level by level, masking the flag bits out of each
//! entry to get the next physical address.
//!
//! We keep it simple by **identity-mapping** physical memory: `phys_to_virt(p)`
//! is just `p`. Because all usable RAM is below 1 GiB (well under 126 MiB), a
//! physical frame — including a page-table frame we just allocated — is always
//! reachable at its own address. That makes the walk read as pure page-table
//! logic, with no offset or recursion machinery in the way. See
//! `docs/concepts/virtual-memory.md`.
//!
//! House style note: no `x86_64` crate — the entry format and the walk are
//! hand-rolled with raw `u64` bit operations, like the rest of the kernel.
//!
//! Re-entrancy: this module keeps no global mutable state of its own — the active
//! address space lives in CR3 and in page-table frames, and the frame allocator
//! guards its own state. `init`/`self_test` run before `sti`, and nothing here is
//! called from an interrupt handler, so no lock is needed yet (a revisit for M9,
//! like the allocator).

use crate::frame_allocator;

/// 4 KiB page / frame size.
const FRAME_SIZE: u64 = 4096;

/// Extracts the physical address from a page-table entry: bits 51..12. This is
/// **not** `!0xfff` — a plain `!0xfff` would leave the NX / available / reserved
/// high bits in the address and send the walk into garbage. Bits 63..52 must be
/// cleared too.
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

// Page-table entry flag bits (low 12 bits of an entry).
/// Present: the entry is valid; touching a not-present entry raises a #PF.
const PRESENT: u64 = 1 << 0;
/// Writable: 0 = read-only, 1 = writable for the region under this entry. Public
/// so callers of [`map_page`] (e.g. the M8 heap mapping writable pages) can name
/// the flag; `map_page` OR-s in `PRESENT` itself.
pub const WRITABLE: u64 = 1 << 1;
/// Page Size: at the PD level, this entry maps a 2 MiB page directly instead of
/// pointing at a page table. (The boot code identity-maps 1 GiB with these.)
const HUGE: u64 = 1 << 7;
/// User/Supervisor: 0 = ring-0-only (the default for every mapping so far,
/// which is *why* ring 3 can't touch kernel memory), 1 = reachable from ring 3.
/// Milestone 13: user pages set this on the leaf **and every table above it** —
/// the CPU ANDs U/S down the whole walk, so one missing bit anywhere denies the
/// access. Public so `usermode` can name the flag; [`map_user_page`] threads it.
pub const USER: u64 = 1 << 2;

/// The physical memory we identity-map: the first 1 GiB. Every physical address
/// the kernel touches (all usable RAM is < 126 MiB, plus VGA at 0xb8000) falls
/// inside this window, so under the identity map a physical address doubles as a
/// dereferenceable virtual address.
const IDENTITY_LIMIT: u64 = 1 << 30;

/// Turn a physical address into a virtual one the kernel can dereference. Under
/// the identity map this is the identity function; the assert keeps the promise
/// *loud even in release builds* — if a frame ever falls outside the mapped
/// window it panics here instead of silently forming a wild pointer that corrupts
/// memory and faults far away. All usable RAM is < 126 MiB, so this never fires
/// today; it guards a future >1 GiB machine or a stray frame. (A plain
/// `debug_assert!` would be compiled out of our `--release` image — no net.)
fn phys_to_virt(phys: u64) -> u64 {
    assert!(
        phys < IDENTITY_LIMIT,
        "paging: physical address outside the identity map"
    );
    phys
}

/// The 9-bit table index selecting an entry for `virt` at `level`
/// (3 = PML4, 2 = PDPT, 1 = PD, 0 = PT).
fn table_index(virt: u64, level: u32) -> usize {
    ((virt >> (12 + 9 * level)) & 0x1FF) as usize
}

/// Read CR3 — the physical address of the active PML4 (low bits are flags).
fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: reading a control register has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Read entry `index` from the page table whose physical frame is `table_phys`.
///
/// SAFETY: `table_phys` must be a real, 4 KiB-aligned page-table frame lying
/// within the identity map, so `phys_to_virt` yields a valid, readable pointer.
/// Callers pass either CR3's PML4 frame or a present next-level pointer they just
/// read from a valid table.
unsafe fn read_entry(table_phys: u64, index: usize) -> u64 {
    let table = phys_to_virt(table_phys) as *const u64;
    // Volatile: the hardware page-walker also reads these, so the compiler must
    // not cache, reorder, or elide the access. (TLB coherence is handled
    // separately — invlpg / CR3 reload — wherever we write an entry.)
    unsafe { core::ptr::read_volatile(table.add(index)) }
}

/// Translate a virtual address to its physical address by walking the page tables
/// currently in CR3, or `None` if the address is not mapped.
pub fn translate(virt: u64) -> Option<u64> {
    translate_from(read_cr3() & ADDR_MASK, virt)
}

/// Walk an arbitrary page-table tree rooted at PML4 frame `pml4_phys` — the same
/// logic as [`translate`], but against a given root rather than the live CR3. Used
/// to verify a freshly-built address space *before* we switch to it. Handles the
/// 2 MiB huge pages the boot tables use (PS bit at the PD level) and, defensively,
/// a 1 GiB huge page at the PDPT level (we never create those, but a correct walk
/// must not misread one).
fn translate_from(pml4_phys: u64, virt: u64) -> Option<u64> {
    let mut table_phys = pml4_phys;

    // Walk PML4 (level 3) -> PDPT (2) -> PD (1), stopping early on a huge page.
    for level in (1..=3).rev() {
        let index = table_index(virt, level);
        // SAFETY: `table_phys` is CR3's PML4 on the first pass, then a present
        // next-level frame we read from a valid table — all within the identity map.
        let entry = unsafe { read_entry(table_phys, index) };

        if entry & PRESENT == 0 {
            return None;
        }
        // A large page (PS bit) ends the walk — but only at the PD (2 MiB) and
        // PDPT (1 GiB) levels. Bit 7 is reserved/ignored at the PML4 level, so we
        // must not misread a stray one there as a bogus 512 GiB page.
        if level <= 2 && entry & HUGE != 0 {
            let offset_bits = 12 + 9 * level; // 21 at PD, 30 at PDPT
            let offset_mask = (1u64 << offset_bits) - 1;
            // Mask the page's low bits (PAT / reserved) out of the base before
            // adding the in-page offset, so a set low bit can't double-count.
            let page = (entry & ADDR_MASK) & !offset_mask;
            return Some(page + (virt & offset_mask));
        }
        table_phys = entry & ADDR_MASK; // descend to the next-level table
    }

    // `table_phys` is now the PT (level 0); its entry maps a 4 KiB page.
    let entry = unsafe { read_entry(table_phys, table_index(virt, 0)) };
    if entry & PRESENT == 0 {
        return None;
    }
    Some((entry & ADDR_MASK) + (virt & 0xFFF))
}

/// Write `entry` at `index` of the page table at physical frame `table_phys`.
///
/// SAFETY: `table_phys` must be a real, 4 KiB-aligned page-table frame within the
/// identity map. The TLB is not consulted here; callers keep it in sync (a fresh
/// CR3 load flushes it; changed live mappings need `invlpg`).
unsafe fn write_entry(table_phys: u64, index: usize, entry: u64) {
    let table = phys_to_virt(table_phys) as *mut u64;
    // Volatile: the page-walker reads these structures, so the store must be
    // materialized, not held in a register or reordered by the compiler.
    unsafe { core::ptr::write_volatile(table.add(index), entry) };
}

/// Zero a freshly allocated 4 KiB frame (512 × `u64`). The frame allocator does
/// **not** zero, and an un-zeroed page-table frame would have random bits read as
/// present entries pointing at garbage — so every new table frame is zeroed first.
fn zero_frame(frame_phys: u64) {
    let ptr = phys_to_virt(frame_phys) as *mut u64;
    for i in 0..(FRAME_SIZE / 8) as usize {
        // SAFETY: `frame_phys` is a just-allocated 4 KiB frame in the identity
        // map; `i` ranges over its 512 u64 slots, all in bounds.
        unsafe { *ptr.add(i) = 0 };
    }
}

/// Allocate one physical frame for a page table and zero it, or `None` if no
/// frame is free. (The frame allocator does not zero.)
fn alloc_table() -> Option<u64> {
    let phys = frame_allocator::alloc()?.start_address();
    zero_frame(phys);
    Some(phys)
}

/// Build a fresh page-table hierarchy that identity-maps `[0, IDENTITY_LIMIT)`
/// (1 GiB) with 2 MiB huge pages, and return the physical address of its PML4.
/// This reproduces the map the bootloader gave us, but in tables *we* allocated
/// and own. One PML4 + one PDPT + one PD frame — the huge pages keep it tiny.
fn build_address_space() -> u64 {
    // The initial address space *must* be built; a shortage of frames this early
    // is unrecoverable, so panic (unlike the runtime map_page path below).
    let pml4 = alloc_table().expect("paging: out of frames building the initial page tables");
    let pdpt = alloc_table().expect("paging: out of frames building the initial page tables");
    let pd = alloc_table().expect("paging: out of frames building the initial page tables");

    // Fill the PD: entry i maps the 2 MiB page at physical i * 2 MiB. 512 entries
    // × 2 MiB = 1 GiB, virtual == physical.
    for i in 0..512u64 {
        let page = i * 0x20_0000; // 2 MiB stride
        // SAFETY: `pd` is our freshly-zeroed, identity-mapped PD frame; `i` < 512.
        unsafe { write_entry(pd, i as usize, page | PRESENT | WRITABLE | HUGE) };
    }
    // SAFETY: `pdpt` and `pml4` are our freshly-zeroed, identity-mapped frames.
    unsafe {
        write_entry(pdpt, 0, pd | PRESENT | WRITABLE);
        write_entry(pml4, 0, pdpt | PRESENT | WRITABLE);
    }
    pml4
}

/// Load `pml4_phys` into CR3, switching the active address space. This flushes
/// the (non-global) TLB.
///
/// SAFETY: the tables rooted at `pml4_phys` must already map — at their current
/// virtual addresses — the instruction pointer, the stack, the GDT, and the IDT.
/// The instant this retires, the CPU fetches the next instruction and every
/// access through the new tables; a single missing page is an immediate silent
/// triple-fault. Our caller verifies the map before calling.
unsafe fn write_cr3(pml4_phys: u64) {
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
    }
}

/// Stand up the kernel's own page tables and switch to them. Call once from
/// `kernel_main`, after the frame allocator is up and the IDT is installed, and
/// before `sti`.
pub fn init() {
    let pml4 = build_address_space();

    // Verify the new tables *before* trusting them with execution. We are still
    // on the boot map, so a wrong mapping panics safely here instead of triple-
    // faulting the instant CR3 changes. A uniform identity map of [0, 1 GiB)
    // means checking a few representative points proves the whole window: the VGA
    // buffer, the kernel image, an address just under 1 GiB, and that 1 GiB
    // itself is unmapped.
    assert_eq!(translate_from(pml4, 0xb8000), Some(0xb8000), "new map: VGA buffer");
    assert_eq!(translate_from(pml4, 0x10_0000), Some(0x10_0000), "new map: kernel image");
    assert_eq!(
        translate_from(pml4, 0x3FFF_F000),
        Some(0x3FFF_F000),
        "new map: near 1 GiB"
    );
    assert_eq!(translate_from(pml4, 0x4000_0000), None, "new map: 1 GiB unmapped");

    // SAFETY: the new PML4 identity-maps [0, 1 GiB), verified above. That window
    // covers this code, the stack, the GDT, and the IDT (all in the kernel image
    // below ~2 MiB), so execution continues seamlessly across the switch.
    unsafe { write_cr3(pml4) };

    crate::serial_println!("M7: paging enabled -- running on kernel-built page tables");
}

/// Invalidate the TLB entry for the page containing `virt`, so the CPU re-walks
/// the tables on the next access instead of trusting a cached translation.
fn flush_tlb(virt: u64) {
    // SAFETY: invlpg only drops a cached translation; it has no other effect. It
    // must not be `nomem` — it changes how memory is subsequently seen.
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags));
    }
}

/// Why a [`map_page`] call can fail. A runtime mapping is a fallible operation —
/// the M8 heap will map pages as it grows and must handle a shortage — so the
/// error is returned rather than panicked (unlike the one-time `init` build).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// No physical frame was free for a needed intermediate page table.
    OutOfFrames,
    /// The virtual address already lies inside a huge page; we do not split them.
    HugePage,
}

/// Map the 4 KiB page at virtual address `virt` to physical address `phys`, with
/// `flags` (PRESENT is added automatically). Allocates and links any missing
/// intermediate tables from the frame allocator. `virt`/`phys` are rounded down
/// to their 4 KiB page. The mapping is **supervisor-only** (U/S=0) — ring 3
/// cannot reach it. Fails with [`MapError::HugePage`] if the address is already
/// inside a huge page (we don't split), or [`MapError::OutOfFrames`] if a table
/// frame can't be allocated.
pub fn map_page(virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    map_inner(virt, phys, flags, false)
}

/// Like [`map_page`], but marks the leaf **and every table on the walk**
/// user-accessible (U/S=1), so code running in ring 3 can touch it — the M13
/// user code and user stack are mapped with this.
///
/// The whole subtlety lives here: the hardware page-walk ANDs the U/S bit at
/// every level, so a user leaf under a supervisor-only PML4/PDPT/PD entry is
/// still unreachable from ring 3 (it would #PF on the very first access). So this
/// sets U/S on the tables it *creates* and OR-s U/S into any table that already
/// exists on the path — notably `PML4[0]`, which is shared with the kernel
/// identity map. That is safe: setting U/S on an upper entry only *permits* user
/// access to continue downward; the kernel's own leaves stay U/S=0, so no kernel
/// page becomes user-readable. (This is exactly what keeps the M15 secret safe.)
///
/// Two properties to know: (1) the U/S loosening of a shared upper entry
/// (`PML4[0]`, and the heap's `PDPT[1]`) is **permanent** — [`unmap_page`] clears
/// only the leaf, never the parents — so once any user page has been mapped, a
/// later M15 secret placed *under* those entries stays protected only by its own
/// U/S=0 leaf; keep it that way. (2) `virt` MUST lie above the 1 GiB identity
/// window: the identity map uses 2 MiB huge pages, so a user VA below 1 GiB would
/// loosen `PML4[0]`/`PDPT[0]` on the way down and *then* fail with
/// [`MapError::HugePage`] at the PD, leaving those shared entries loosened for a
/// call that returned an error. The assert makes that misuse loud instead.
pub fn map_user_page(virt: u64, phys: u64, flags: u64) -> Result<(), MapError> {
    assert!(
        virt >= IDENTITY_LIMIT,
        "map_user_page: user VA {:#x} must be above the 1 GiB identity window",
        virt
    );
    map_inner(virt, phys, flags, true)
}

/// Shared walk for [`map_page`] / [`map_user_page`]. When `user`, U/S=1 is added
/// to created intermediates, OR-ed into existing intermediates on the path, and
/// set on the leaf; when not, behaviour is the original supervisor-only mapping.
fn map_inner(virt: u64, phys: u64, flags: u64, user: bool) -> Result<(), MapError> {
    let virt = virt & !(FRAME_SIZE - 1);
    let phys = phys & !(FRAME_SIZE - 1);
    let mut table_phys = read_cr3() & ADDR_MASK;
    let new_table = if user {
        PRESENT | WRITABLE | USER
    } else {
        PRESENT | WRITABLE
    };

    // Descend PML4 -> PDPT -> PD, creating any missing table, to reach the PT.
    for level in (1..=3).rev() {
        let index = table_index(virt, level);
        // SAFETY: `table_phys` is CR3's PML4, then a present next-level frame we
        // just read or just allocated — all in the identity map.
        let entry = unsafe { read_entry(table_phys, index) };
        if entry & PRESENT != 0 {
            // A huge page at PD (2 MiB) or PDPT (1 GiB) has no lower table to
            // descend into; bit 7 is meaningless at PML4, so only check level <= 2.
            if level <= 2 && entry & HUGE != 0 {
                return Err(MapError::HugePage);
            }
            // For a user mapping, an already-present intermediate (e.g. the shared
            // PML4[0]) may lack U/S=1 — OR it in so the walk can reach the user
            // leaf. Only ever *adds* a permission; never touches the address bits.
            if user && entry & USER == 0 {
                // SAFETY: `table_phys` is the identity-mapped table we just read
                // `entry` from; we rewrite the same slot with one extra flag bit.
                unsafe { write_entry(table_phys, index, entry | USER) };
            }
            table_phys = entry & ADDR_MASK;
        } else {
            let next = alloc_table().ok_or(MapError::OutOfFrames)?;
            // SAFETY: `table_phys` is a valid, identity-mapped table frame.
            unsafe { write_entry(table_phys, index, next | new_table) };
            table_phys = next;
        }
    }

    // `table_phys` is the PT; install the leaf entry.
    let leaf = phys | flags | PRESENT | if user { USER } else { 0 };
    // SAFETY: `table_phys` is the identity-mapped PT frame for this address.
    unsafe { write_entry(table_phys, table_index(virt, 0), leaf) };
    flush_tlb(virt);
    Ok(())
}

/// Unmap the 4 KiB page at `virt` by clearing its PT entry; returns `false` if it
/// was not mapped. Intermediate tables are left in place (a minor, deliberate
/// leak — reclaiming empty tables is a later refinement).
pub fn unmap_page(virt: u64) -> bool {
    let virt = virt & !(FRAME_SIZE - 1);
    let mut table_phys = read_cr3() & ADDR_MASK;

    for level in (1..=3).rev() {
        let index = table_index(virt, level);
        // SAFETY: `table_phys` is a present, identity-mapped table frame.
        let entry = unsafe { read_entry(table_phys, index) };
        if entry & PRESENT == 0 || entry & HUGE != 0 {
            return false; // unmapped, or inside a huge page we won't split
        }
        table_phys = entry & ADDR_MASK;
    }

    let index = table_index(virt, 0);
    // SAFETY: identity-mapped PT frame.
    let entry = unsafe { read_entry(table_phys, index) };
    if entry & PRESENT == 0 {
        return false;
    }
    unsafe { write_entry(table_phys, index, 0) };
    flush_tlb(virt);
    true
}

/// Prove map / translate / unmap work, over serial. The honesty check: the test
/// maps a fresh frame at a virtual address **above** the 1 GiB identity window,
/// so a successful read/write there can only come from our own new mapping — an
/// already-identity-mapped address would prove nothing.
pub fn self_test() {
    const TEST_VIRT: u64 = 0x4000_0000; // exactly 1 GiB — first address past the map
    const SENTINEL: u32 = 0xDEAD_BEEF;

    let free_before = frame_allocator::free_frame_count();

    // It must start unmapped (the software walk agrees with the hardware).
    assert_eq!(translate(TEST_VIRT), None, "paging self-test: address should start unmapped");

    let frame = frame_allocator::alloc().expect("paging self-test: no free frame");
    let phys = frame.start_address();
    map_page(TEST_VIRT, phys, WRITABLE).expect("paging self-test: map_page failed");

    // Write through the new virtual address; read back through the frame's own
    // identity address. Equal => TEST_VIRT and `phys` alias the same physical RAM.
    // SAFETY: TEST_VIRT is now mapped writable to `phys`; `phys` (< 126 MiB) is
    // readable through the identity map. Both point at the same real frame.
    unsafe { core::ptr::write_volatile(TEST_VIRT as *mut u32, SENTINEL) };
    let via_phys = unsafe { core::ptr::read_volatile(phys as *const u32) };
    assert_eq!(via_phys, SENTINEL, "map_page: virtual and physical must alias the same frame");
    assert_eq!(translate(TEST_VIRT), Some(phys), "translate must resolve the new mapping");
    crate::serial_println!(
        "[ok] paging: mapped {:#014x} -> {:#014x}, round-tripped {:#x} above the 1 GiB map",
        TEST_VIRT,
        phys,
        SENTINEL
    );

    // Unmap and confirm the walk now reports it gone.
    assert!(unmap_page(TEST_VIRT), "unmap should succeed on a mapped page");
    assert_eq!(translate(TEST_VIRT), None, "address should be unmapped again");
    crate::serial_println!("[ok] paging: unmapped {:#014x}", TEST_VIRT);

    frame_allocator::free(frame);

    // map_page allocated two intermediate tables (a PD for the empty PDPT[1], and
    // its PT); unmap_page clears only the leaf, so those two frames are a
    // deliberate, bounded leak. Assert exactly that, so it stays visible and
    // intentional rather than silent accounting drift — reclaiming empty tables is
    // a later refinement.
    assert_eq!(
        frame_allocator::free_frame_count(),
        free_before - 2,
        "paging self-test should leak exactly the two intermediate tables"
    );
    crate::serial_println!("[ok] paging: self-test left 2 intermediate tables mapped (expected)");
}

/// Milestone 13 (step 2): prove [`map_user_page`] marks U/S=1 at **every level**
/// of the walk, not just the leaf — the one subtlety that decides whether ring-3
/// code can reach the page or #PFs on its first access. Maps a fresh frame at a
/// user VA, then re-walks the live tables and asserts PRESENT|USER on the PML4,
/// PDPT, PD, and PT entries plus the leaf, and round-trips a sentinel through it.
///
/// Uses 2 GiB — clear of both the identity window (< 1 GiB) and the M8 heap
/// region (mapped at 1 GiB) and self-test's own 1 GiB subtree — so its two-frame
/// intermediate leak is deterministic no matter what ran before it.
pub fn user_map_self_test() {
    const UVA: u64 = 2 * (1 << 30); // 2 GiB
    const SENTINEL: u32 = 0x5EED_C0DE;

    let free_before = frame_allocator::free_frame_count();
    assert_eq!(translate(UVA), None, "user-map self-test: address should start unmapped");

    let frame = frame_allocator::alloc().expect("user-map self-test: no free frame");
    let phys = frame.start_address();
    map_user_page(UVA, phys, WRITABLE).expect("user-map self-test: map_user_page failed");

    // The whole point: U/S=1 at PML4 -> PDPT -> PD (the tables on the path)...
    let mut table_phys = read_cr3() & ADDR_MASK;
    for level in (1..=3).rev() {
        // SAFETY: `table_phys` is CR3's PML4, then a present next-level frame we
        // just read — all within the identity map.
        let entry = unsafe { read_entry(table_phys, table_index(UVA, level)) };
        assert!(entry & PRESENT != 0, "user walk: level {} not present", level);
        assert!(
            entry & USER != 0,
            "user walk: level {} missing U/S — ring 3 would #PF before reaching the leaf",
            level
        );
        table_phys = entry & ADDR_MASK;
    }
    // ...and on the leaf itself.
    // SAFETY: `table_phys` is the identity-mapped PT frame for UVA.
    let leaf = unsafe { read_entry(table_phys, table_index(UVA, 0)) };
    assert!(
        leaf & (PRESENT | USER) == (PRESENT | USER),
        "user leaf missing PRESENT|USER: {:#x}",
        leaf
    );

    // And it still aliases its frame like any mapping (write via UVA, read via phys).
    // SAFETY: UVA is mapped writable to `phys`; `phys` (< 126 MiB) is readable
    // through the identity map. Both name the same real frame.
    unsafe { core::ptr::write_volatile(UVA as *mut u32, SENTINEL) };
    let via_phys = unsafe { core::ptr::read_volatile(phys as *const u32) };
    assert_eq!(via_phys, SENTINEL, "user page must alias its frame");
    crate::serial_println!(
        "[ok] paging: user page {:#014x} has U/S=1 at all 4 levels + leaf — reachable from ring 3",
        UVA
    );

    assert!(unmap_page(UVA), "unmap should succeed on the mapped user page");
    frame_allocator::free(frame);
    // Like `self_test`, map_user_page allocated a PD + PT (2 frames) for the fresh
    // PDPT[2] subtree; unmap clears only the leaf, so those two are a bounded,
    // deliberate leak. (OR-ing U/S into the pre-existing PML4[0] cost no frame.)
    assert_eq!(
        frame_allocator::free_frame_count(),
        free_before - 2,
        "user-map self-test should leak exactly the two intermediate tables"
    );
    crate::serial_println!("[ok] paging: user-map self-test left 2 intermediate tables mapped (expected)");
}
