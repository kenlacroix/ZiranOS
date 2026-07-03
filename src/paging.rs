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

/// 4 KiB page / frame size. (Used by the table-building steps that follow.)
#[allow(dead_code)]
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
/// (Used by `map_page` in the table-building steps that follow.)
#[allow(dead_code)]
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
/// currently in CR3, or `None` if the address is not mapped. Handles the 2 MiB
/// huge pages the boot tables use (PS bit at the PD level) and, defensively, a
/// 1 GiB huge page at the PDPT level (we never create those, but a correct walk
/// must not misread one).
pub fn translate(virt: u64) -> Option<u64> {
    let mut table_phys = read_cr3() & ADDR_MASK; // the PML4 frame

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
