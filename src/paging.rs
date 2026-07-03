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
/// Writable: 0 = read-only, 1 = writable for the region under this entry.
const WRITABLE: u64 = 1 << 1;
/// Page Size: at the PD level, this entry maps a 2 MiB page directly instead of
/// pointing at a page table. (The boot code identity-maps 1 GiB with these.)
const HUGE: u64 = 1 << 7;

/// The physical memory we identity-map: the first 1 GiB. Every physical address
/// the kernel touches (all usable RAM is < 126 MiB, plus VGA at 0xb8000) falls
/// inside this window, so under the identity map a physical address doubles as a
/// dereferenceable virtual address.
const IDENTITY_LIMIT: u64 = 1 << 30;

/// Turn a physical address into a virtual one the kernel can dereference. Under
/// the identity map this is the identity function; the assert makes the
/// assumption *loud* if a frame ever falls outside the mapped window, rather than
/// letting it trip a silent fault later.
fn phys_to_virt(phys: u64) -> u64 {
    debug_assert!(
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
    // A page table is ordinary RAM (not MMIO); a plain read is correct. The CPU
    // also reads these; we keep the TLB in sync explicitly (invlpg / CR3 reload)
    // wherever we *write* them.
    unsafe { *table.add(index) }
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
        if entry & HUGE != 0 {
            // A large page ends the walk here. Level 1 = 2 MiB (21-bit offset),
            // level 2 = 1 GiB (30-bit offset).
            let offset_bits = 12 + 9 * level; // 21 at PD, 30 at PDPT
            let page = entry & ADDR_MASK;
            let offset_mask = (1u64 << offset_bits) - 1;
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
    unsafe { *table.add(index) = entry };
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

/// Allocate one physical frame for a page table and zero it. Panics on
/// exhaustion — we cannot build an address space without frames.
fn alloc_table() -> u64 {
    let frame = frame_allocator::alloc().expect("paging: out of frames building page tables");
    let phys = frame.start_address();
    zero_frame(phys);
    phys
}

/// Build a fresh page-table hierarchy that identity-maps `[0, IDENTITY_LIMIT)`
/// (1 GiB) with 2 MiB huge pages, and return the physical address of its PML4.
/// This reproduces the map the bootloader gave us, but in tables *we* allocated
/// and own. One PML4 + one PDPT + one PD frame — the huge pages keep it tiny.
fn build_address_space() -> u64 {
    let pml4 = alloc_table();
    let pdpt = alloc_table();
    let pd = alloc_table();

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

/// Map the 4 KiB page at virtual address `virt` to physical address `phys`, with
/// `flags` (PRESENT is added automatically). Allocates and links any missing
/// intermediate tables from the frame allocator. `virt`/`phys` are rounded down
/// to their 4 KiB page. Only maps into virtual space not already covered by a
/// huge page (it is meant for fresh addresses above the identity window).
pub fn map_page(virt: u64, phys: u64, flags: u64) {
    let virt = virt & !(FRAME_SIZE - 1);
    let phys = phys & !(FRAME_SIZE - 1);
    let mut table_phys = read_cr3() & ADDR_MASK;

    // Descend PML4 -> PDPT -> PD, creating any missing table, to reach the PT.
    for level in (1..=3).rev() {
        let index = table_index(virt, level);
        // SAFETY: `table_phys` is CR3's PML4, then a present next-level frame we
        // just read or just allocated — all in the identity map.
        let entry = unsafe { read_entry(table_phys, index) };
        if entry & PRESENT != 0 {
            assert!(
                entry & HUGE == 0,
                "map_page: virtual address is inside an existing huge page"
            );
            table_phys = entry & ADDR_MASK;
        } else {
            let next = alloc_table();
            // SAFETY: `table_phys` is a valid, identity-mapped table frame.
            unsafe { write_entry(table_phys, index, next | PRESENT | WRITABLE) };
            table_phys = next;
        }
    }

    // `table_phys` is the PT; install the leaf entry.
    // SAFETY: `table_phys` is the identity-mapped PT frame for this address.
    unsafe { write_entry(table_phys, table_index(virt, 0), phys | flags | PRESENT) };
    flush_tlb(virt);
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

    // It must start unmapped (the software walk agrees with the hardware).
    assert_eq!(translate(TEST_VIRT), None, "paging self-test: address should start unmapped");

    let frame = frame_allocator::alloc().expect("paging self-test: no free frame");
    let phys = frame.start_address();
    map_page(TEST_VIRT, phys, WRITABLE);

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
}
