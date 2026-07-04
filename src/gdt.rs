//! Global Descriptor Table + Task State Segment — Milestone 13 groundwork,
//! "the jail needs walls before a prisoner."
//!
//! The boot GDT (`boot/boot.asm`) held exactly two descriptors: the null entry
//! and one flat ring-0 code segment. That was all long mode needed to run the
//! kernel. But ring 3 — the first real privilege boundary (M13) — needs more:
//! a **user code** and **user data** descriptor marked DPL 3 so the CPU will
//! *let* unprivileged code run under them, and a **Task State Segment (TSS)**
//! whose `RSP0` field tells the CPU which kernel stack to switch to the instant
//! an interrupt or syscall drags execution from ring 3 back down to ring 0.
//! Without a loaded TSS, that first ring 3 → ring 0 transition has nowhere to
//! push the interrupt frame → #DF → triple fault → silent reboot (see the M13
//! eng-plan, Q5). So this table is a precondition for *any* ring-3 code.
//!
//! The boot GDT lives in read-only `.rodata` and can't hold a TSS descriptor,
//! whose base is a runtime Rust address unknown at assembly time. So we rebuild
//! the whole GDT here, in Rust, mirroring how `interrupts.rs` builds the IDT:
//! fill a static table, `lgdt` it, reload CS, load the data selectors, `ltr` the
//! TSS. From this point on the kernel owns its descriptor tables end to end.
//!
//! See `docs/concepts/privilege.md` (written during the M13 build) for rings,
//! the TSS, and the U/S bit from first principles.

use crate::serial_println;

// --- Segment selectors (a byte offset into the GDT | the requested RPL) -------
// These offsets are load-bearing: `usermode.rs` and the syscall gate hardcode
// the ring-3 forms (0x1b, 0x23), so the descriptors MUST sit at these offsets.

/// Ring-0 code. Same value the boot GDT used (`interrupts.rs::KERNEL_CODE_SELECTOR`).
pub const KERNEL_CODE: u16 = 0x08;
/// Ring-0 data/stack. Fills 0x10 so the user descriptors land where M13 expects.
pub const KERNEL_DATA: u16 = 0x10;
/// Ring-3 code. Used from ring 3 as `0x18 | 3 = 0x1b`. The descriptor exists now
/// (so the ring-0 boot path is unchanged and testable); the `iretq` launch that
/// consumes this selector arrives in the next M13 step.
#[allow(dead_code)]
pub const USER_CODE: u16 = 0x18;
/// Ring-3 data/stack. Used from ring 3 as `0x20 | 3 = 0x23`. Consumed by the
/// ring-3 launch in the next M13 step; the descriptor is installed now.
#[allow(dead_code)]
pub const USER_DATA: u16 = 0x20;
/// The 64-bit TSS descriptor. 16 bytes wide, so it occupies GDT slots 5 and 6.
pub const TSS_SELECTOR: u16 = 0x28;

/// null, k-code, k-data, u-code, u-data, then the TSS descriptor (two slots).
const GDT_LEN: usize = 7;

/// A dedicated 16 KiB kernel stack. Like the boot stack and the M9 task stacks,
/// it has **no guard page** — an overflow corrupts adjacent `.bss` silently. The
/// syscall/fault path off `RSP0` is shallow (one interrupt frame + a short
/// dispatch), so a single ring-3 excursion fits; a per-task guarded `RSP0` is a
/// later refinement (same debt as the scheduler stacks).
#[repr(C, align(16))]
struct KernelStack([u8; 16 * 1024]);

/// The stack the CPU loads into RSP when an interrupt/syscall enters ring 0 from
/// ring 3 (TSS.RSP0). Static, so it lives for the whole life of the kernel.
static mut PRIV_STACK: KernelStack = KernelStack([0; 16 * 1024]);
/// A second known-good stack for IST1 — a safety net we can point the #DF (and,
/// if needed, #GP/#PF) gate at so a fault taken while RSP0 is momentarily bad
/// still has a stack to land on. Wired into the TSS now; the IDT gate that uses
/// it is a later step (the `ist` field in `interrupts.rs` is still 0).
static mut DF_STACK: KernelStack = KernelStack([0; 16 * 1024]);

/// The 64-bit Task State Segment. In long mode it no longer holds a task's
/// register state (hardware task switching is gone); it survives almost entirely
/// for `RSP0` — the ring-0 stack pointer loaded on a privilege change — plus the
/// IST table. Exactly 104 bytes; `packed` so every field sits at its
/// architecturally-fixed offset (RSP0 at byte 4).
#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist1: u64,
    ist2: u64,
    ist3: u64,
    ist4: u64,
    ist5: u64,
    ist6: u64,
    ist7: u64,
    reserved2: u64,
    reserved3: u16,
    /// Offset of the I/O permission bitmap. Set to the TSS size ⇒ "no bitmap",
    /// so ring 3 has no I/O port access (it must go through a syscall).
    iomap_base: u16,
}

impl Tss {
    const fn new() -> Self {
        Tss {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist1: 0,
            ist2: 0,
            ist3: 0,
            ist4: 0,
            ist5: 0,
            ist6: 0,
            ist7: 0,
            reserved2: 0,
            reserved3: 0,
            iomap_base: 0,
        }
    }
}

/// The GDT: `GDT_LEN` raw 64-bit descriptor slots. A `[u64; _]` is 8-byte
/// aligned, which is all `lgdt` needs (the SDM only recommends 8-byte alignment
/// for the GDT base).
static mut GDT: [u64; GDT_LEN] = [0; GDT_LEN];
/// Our single, static TSS — `lgdt`/`ltr` store only *pointers* to these, so the
/// memory must never move or be freed. `static` guarantees that.
static mut TSS: Tss = Tss::new();

/// The value handed to `lgdt`: the table's size (minus one, a hardware quirk)
/// and its base. Packed so the two fields are adjacent, exactly as the CPU reads.
#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

/// Build the 64-bit *system* descriptor for a TSS (type 0x9, "available 64-bit
/// TSS", present, DPL 0). It is 16 bytes — twice a normal descriptor — with the
/// base address scattered across five fields, another 16/32-bit-era scar.
const fn tss_descriptor(base: u64, limit: u64) -> (u64, u64) {
    let mut low: u64 = 0;
    low |= limit & 0xffff; // limit 15:0
    low |= (base & 0xffff) << 16; // base 15:0
    low |= ((base >> 16) & 0xff) << 32; // base 23:16
    low |= 0x9 << 40; // type = available 64-bit TSS
    low |= 1 << 47; // present
    low |= ((limit >> 16) & 0xf) << 48; // limit 19:16
    low |= ((base >> 24) & 0xff) << 56; // base 31:24
    let high: u64 = (base >> 32) & 0xffff_ffff; // base 63:32
    (low, high)
}

/// Rebuild the GDT in Rust, load it, reload the segment registers, and load the
/// TSS. Safe to call once, early, with interrupts disabled — it replaces the
/// boot GDT wholesale, so it must run before any code depends on a descriptor
/// this table doesn't provide (the user segments and the TSS).
pub fn init() {
    // SAFETY: single-threaded, interrupts still disabled; we are the only writer
    // of GDT/TSS/the stacks, and the asm below installs them atomically enough
    // for a single CPU (lgdt then a CS reload, then ltr, with nothing reentrant
    // in between). Every descriptor bit pattern is the SDM-defined encoding.
    unsafe {
        // Point RSP0 (and the IST1 safety net) at the *tops* of the dedicated
        // stacks — x86 stacks grow downward. 16-byte aligned by construction.
        let priv_top = core::ptr::addr_of!(PRIV_STACK) as u64 + core::mem::size_of::<KernelStack>() as u64;
        let df_top = core::ptr::addr_of!(DF_STACK) as u64 + core::mem::size_of::<KernelStack>() as u64;
        let tss = &mut *core::ptr::addr_of_mut!(TSS);
        tss.rsp0 = priv_top;
        tss.ist1 = df_top;
        tss.iomap_base = core::mem::size_of::<Tss>() as u16; // "no I/O bitmap"

        let tss_base = core::ptr::addr_of!(TSS) as u64;
        let tss_limit = (core::mem::size_of::<Tss>() - 1) as u64;
        let (tss_lo, tss_hi) = tss_descriptor(tss_base, tss_limit);

        let gdt = &mut *core::ptr::addr_of_mut!(GDT);
        gdt[0] = 0; // mandatory null descriptor
        // Ring-0 code: executable(43) | type(44) | present(47) | 64-bit(53).
        gdt[1] = (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53);
        // Ring-0 data: writable(41) | type(44) | present(47).
        gdt[2] = (1 << 41) | (1 << 44) | (1 << 47);
        // Ring-3 code: same as ring-0 code but DPL 3 (45..46 = 3).
        gdt[3] = (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53) | (3 << 45);
        // Ring-3 data: writable data, present, DPL 3.
        gdt[4] = (1 << 41) | (1 << 44) | (1 << 47) | (3 << 45);
        gdt[5] = tss_lo;
        gdt[6] = tss_hi;

        let pointer = GdtPointer {
            limit: (core::mem::size_of::<[u64; GDT_LEN]>() - 1) as u16,
            base: core::ptr::addr_of!(GDT) as u64,
        };
        core::arch::asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));

        // Reload CS. The selector value (0x08) is unchanged, but its cached
        // descriptor still points into the *old* boot GDT until a far transfer
        // reloads it — so we far-return through our own kernel-code descriptor.
        // A far return pops CS:RIP; we push the selector and the address of a
        // local label, then `retfq` lands on it running under the new CS.
        core::arch::asm!(
            "push {sel}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) KERNEL_CODE as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );

        // Load the ring-0 data selector into the data/stack segment registers.
        // Long mode treats these as flat, but SS holding a valid DPL-0 descriptor
        // keeps the ring-0 side well-formed for the privilege transitions to come.
        core::arch::asm!(
            "mov ds, {sel:x}",
            "mov es, {sel:x}",
            "mov ss, {sel:x}",
            sel = in(reg) KERNEL_DATA,
            options(nostack, preserves_flags),
        );

        // Load the task register with the TSS selector — this is what makes RSP0
        // live. From here the CPU knows which stack to switch to on a ring 3 → 0
        // transition.
        core::arch::asm!("ltr {sel:x}", sel = in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
    }

    serial_println!(
        "[ok] GDT rebuilt in Rust: null, k-code(0x08), k-data(0x10), u-code(0x18,DPL3), u-data(0x20,DPL3), TSS(0x28)"
    );
}

/// Prove the descriptor tables are *actually* ours, not accidentally still the
/// boot GDT — the M13 "working vs. accidentally working" discipline. Reads the
/// task register (`str`): if the TSS is loaded, `tr` reads back 0x28. Also
/// confirms RSP0 is a plausible, 16-aligned kernel stack pointer.
pub fn self_test() {
    let tr: u16;
    // SAFETY: `str` reads the task register into a GPR; no memory or flag effects.
    unsafe {
        core::arch::asm!("str {0:x}", out(reg) tr, options(nomem, nostack, preserves_flags));
    }
    assert_eq!(tr, TSS_SELECTOR, "TSS not loaded: str returned {:#x}", tr);

    // Read RSP0 back out of the (packed) TSS without forming an unaligned ref.
    let rsp0 = unsafe { core::ptr::addr_of!(TSS).cast::<u8>().add(4).cast::<u64>().read_unaligned() };
    assert!(rsp0 != 0, "TSS.RSP0 is null");
    assert!(rsp0 % 16 == 0, "TSS.RSP0 not 16-byte aligned: {:#x}", rsp0);

    serial_println!(
        "[ok] GDT/TSS self-test: tr={:#06x}, RSP0={:#018x} (16-aligned) — descriptor tables are ours",
        tr,
        rsp0
    );
}
