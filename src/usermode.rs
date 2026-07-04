//! Userspace / ring 3 — Milestone 13, "the first wall the kernel defends."
//!
//! Everything before this milestone ran in ring 0 with full power; there was no
//! *inside* and *outside*. This module runs one tiny program in **ring 3**, where
//! the hardware — not the kernel's goodwill — forbids it from touching kernel
//! memory or executing privileged instructions. Its only way to ask the kernel
//! for anything is the `int 0x80` syscall gate (`interrupts.rs`).
//!
//! The one lesson: **a privilege boundary is enforced by the CPU, not the
//! kernel.** We never write an `if` that checks "is this code allowed to do
//! that." We set two bits — the descriptor's DPL (`gdt.rs`) and the page's U/S
//! (`paging.rs`) — and from then on the silicon faults *for* us. The proof is not
//! "it printed and didn't crash"; it is that the syscall arrives with the saved
//! `CS` showing **CPL 3** (asserted in [`resume_kernel`]), so the CPU genuinely
//! was in ring 3.
//!
//! The excursion (see `boot/usermode.asm` for the asm half):
//! ```text
//!   self_test: map the blob user-accessible, map a user stack, then:
//!     usermode_enter  -- save kernel regs, fabricate an iretq frame, drop to CPL 3
//!       blob: mov eax,SYS_PRINT; mov edi,'Z'; int 0x80   -> kernel prints 'Z'
//!             mov eax,SYS_EXIT;               int 0x80   -> resume_kernel rewrites
//!                                                            the frame back to ring 0
//!     usermode_resume -- unwind saved regs, return into self_test (ring 0)
//! ```
//!
//! Deliberately minimal (M13 scope-guard "Hold"): one blob, one address space,
//! args passed in registers (no user pointer to validate yet — that, and
//! copy-from-user, is the M15 "confused deputy" surface), no ELF, no loader, no
//! scheduler integration. See `docs/planning/milestone-13-eng-plan.md`.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::frame_allocator::PhysFrame;
use crate::interrupts::InterruptContext;
use crate::{frame_allocator, gdt, interrupts, paging, serial_println};

// User virtual addresses: above the 1 GiB identity window, and clear of the M8
// heap (at 1 GiB) and the paging self-tests (1 GiB / 2 GiB). One 4 KiB page each.
const UVA_CODE: u64 = 0x5000_0000; // 1.25 GiB
const UVA_STACK: u64 = 0x5010_0000; // a separate page, 1 MiB above the code
const UVA_STACK_TOP: u64 = UVA_STACK + 0x1000; // 16-byte aligned top of the stack

/// The ring-3 program, as raw machine code. **Position-independent** — only
/// immediate-into-register moves and `int 0x80`, no absolute addresses — so it
/// runs correctly from the relocated virtual address it is copied to. `mov eax`
/// zero-extends into rax, so the syscall number lands in the full register.
///
/// ```text
///   mov eax, 1      ; SYS_PRINT
///   mov edi, 0x5A   ; 'Z'
///   int 0x80        ; -> kernel prints 'Z', returns here in ring 3
///   mov eax, 2      ; SYS_EXIT
///   int 0x80        ; -> kernel takes over; does not return to the blob
///   jmp $           ; safety net if SYS_EXIT ever did return
/// ```
static USER_BLOB: [u8; 21] = [
    0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1   (SYS_PRINT)
    0xBF, 0x5A, 0x00, 0x00, 0x00, // mov edi, 0x5A ('Z')
    0xCD, 0x80, // int 0x80
    0xB8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2   (SYS_EXIT)
    0xCD, 0x80, // int 0x80
    0xEB, 0xFE, // jmp $ (spin — must not be reached)
];

extern "C" {
    /// Drop to ring 3 at `entry` on `user_stack_top`; returns only via SYS_EXIT.
    fn usermode_enter(entry: u64, user_stack_top: u64);
    /// A code *label* the SYS_EXIT frame-rewrite points at — we only take its
    /// address, never call it from Rust.
    fn usermode_resume();
    /// The kernel RSP `usermode_enter` stashed before dropping to ring 3.
    static kernel_rsp: u64;
}

/// Called from the SYS_EXIT syscall arm. Rewrite the saved interrupt frame so the
/// ISR's own `iretq` (in `isr_common`) returns to the **kernel** at
/// `usermode_resume` — on the stack `usermode_enter` stashed — instead of back to
/// the ring-3 blob. This is the return half of the excursion; it rides the
/// existing ISR path rather than adding a second mechanism.
///
/// The `CS`-is-CPL-3 assertion is the M13 gate: SYS_EXIT must only ever be
/// serviced for a genuine ring-3 caller. If the blob had secretly still been in
/// ring 0 (a mis-wired iretq frame), this fires instead of silently "working."
pub fn resume_kernel(ctx: &mut InterruptContext) {
    assert!(
        ctx.cs & 3 == 3,
        "SYS_EXIT serviced from CPL {} — the excursion was not really in ring 3",
        ctx.cs & 3
    );
    ctx.rip = usermode_resume as *const () as u64;
    ctx.cs = gdt::KERNEL_CODE as u64;
    ctx.ss = gdt::KERNEL_DATA as u64;
    ctx.rflags = 0x2; // reserved bit 1 only; IF=0 (the caller re-enables via restore)
    // SAFETY: `kernel_rsp` is the u64 the asm wrote before the iretq to ring 3.
    ctx.rsp = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(kernel_rsp)) };
}

/// Milestone 13: run ONE user program in ring 3, have it make a syscall from
/// CPL 3, and return to the kernel. The visible proof is the `'Z'` the blob asks
/// `SYS_PRINT` to draw and the serial line showing `CS=0x1b, CPL=3`; the hard
/// gate is [`resume_kernel`]'s CPL-3 assertion. Reaching the end at all proves
/// the ring 0 → 3 → 0 round-trip left the kernel's GDT/IDT/CR3/stack intact.
pub fn self_test() {
    // Map the blob user-accessible + executable and a user stack (see helper).
    let (code_frame, stack_frame) = map_user_program(&USER_BLOB);

    serial_println!(
        "[m13] entering ring 3: code@{:#x}, stack@{:#x} (expect CS=0x1b, CPL=3)",
        UVA_CODE,
        UVA_STACK_TOP
    );

    // The excursion must be atomic w.r.t. the timer. Ring 3 runs with IF=0, but
    // the ring-0 setup/return runs after `sti`, so a tick could otherwise preempt
    // `usermode_enter` mid-frame — and there is no ring-3 save/restore path yet
    // (an explicit M13 deferral). Disable interrupts around the whole round-trip.
    let was = interrupts::save_and_disable();
    // SAFETY: the GDT (ring-3 selectors + TSS.RSP0), the DPL-3 `int 0x80` gate,
    // and the user mappings are all installed. `usermode_enter` fabricates a valid
    // iretq frame and only returns via SYS_EXIT's rewrite; interrupts are off.
    unsafe {
        usermode_enter(UVA_CODE, UVA_STACK_TOP);
    }
    interrupts::restore(was);

    serial_println!("[ok] returned to ring 0 from the userspace excursion — round-trip intact");

    unmap_user_program(code_frame, stack_frame);

    serial_println!("M13: userspace online");
}

/// The vector of the last ring-3 violation the enforcement test caught (13=#GP,
/// 14=#PF), or 0 for none. Written by [`recover_from_violation`] from the fault
/// handler, read by [`run_violation`] after the excursion returns.
static LAST_VIOLATION: AtomicU64 = AtomicU64::new(0);

/// Blob: `cli` (a privileged instruction) then spin. In ring 3 with IOPL 0,
/// `cli` raises **#GP** — the CPU refuses to let unprivileged code disable
/// interrupts.
static BLOB_CLI: [u8; 3] = [
    0xFA, // cli
    0xEB, 0xFE, // jmp $ (never reached — cli faults first)
];

/// Blob: read the VGA buffer at absolute `0xb8000` (a kernel-only, U/S=0 page)
/// then spin. From ring 3 this raises **#PF** with the error-code U/S bit set —
/// the page permission stopping a user read of kernel memory.
static BLOB_READ_KERNEL: [u8; 12] = [
    0x48, 0xA1, 0x00, 0x80, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, // mov rax, [0xb8000]
    0xEB, 0xFE, // jmp $ (never reached — the read faults first)
];

/// Called from the ring-3 fault arm of `interrupt_dispatch`. Record which fault
/// the CPU raised, then rewrite the frame to unwind back to ring 0 — same
/// mechanism as [`resume_kernel`], so a caught violation returns cleanly into
/// [`run_violation`] instead of halting the kernel.
pub fn recover_from_violation(ctx: &mut InterruptContext, vector: u64) {
    LAST_VIOLATION.store(vector, Ordering::SeqCst);
    resume_kernel(ctx);
}

/// Milestone 13 — the enforcement test (the M15 privilege-boundary preview). Run
/// two blobs that each *deliberately* violate the boundary and assert the CPU
/// caught each one, from CPL 3: a privileged instruction (`cli` → #GP) and a read
/// of a kernel-only page (`0xb8000` → #PF, U/S bit set). "It printed and didn't
/// crash" was never the bar; "it tried to cheat and the hardware said no, from
/// ring 3" is (office-hours Q4).
pub fn enforcement_test() {
    run_violation("cli (a privileged instruction)", &BLOB_CLI, 13);
    run_violation("a read of kernel page 0xb8000", &BLOB_READ_KERNEL, 14);
    serial_println!(
        "[ok] M13: ring-3 boundary enforced — privileged instr -> #GP, kernel read -> #PF, both caught at CPL 3"
    );
}

/// Run one violating blob in ring 3 and assert the CPU raised `expect_vector`
/// (13=#GP, 14=#PF) — proof the boundary held. The fault handler unwinds us back
/// here; if the blob had somehow *not* faulted, `usermode_enter` would spin in
/// the blob's `jmp $` and this would hang (a visible failure), so returning at
/// all already means a fault was taken.
fn run_violation(what: &str, blob: &[u8], expect_vector: u64) {
    let (code_frame, stack_frame) = map_user_program(blob);
    LAST_VIOLATION.store(0, Ordering::SeqCst);
    serial_println!("[m13] ring 3 will attempt: {} — expecting the CPU to fault", what);

    let was = interrupts::save_and_disable();
    // SAFETY: same preconditions as `self_test`'s launch; the violation is caught
    // by the ring-3 fault arm, which unwinds via `recover_from_violation`.
    unsafe {
        usermode_enter(UVA_CODE, UVA_STACK_TOP);
    }
    interrupts::restore(was);

    let caught = LAST_VIOLATION.load(Ordering::SeqCst);
    assert_eq!(
        caught, expect_vector,
        "ring-3 {} should have raised vector {}, but the caught vector was {}",
        what, expect_vector, caught
    );
    unmap_user_program(code_frame, stack_frame);
}

/// Map a blob's code (PRESENT|USER, executable, not writable) at `UVA_CODE` and a
/// writable user stack at `UVA_STACK`, returning the backing frames. Shared by
/// the syscall excursion and the enforcement test.
fn map_user_program(blob: &[u8]) -> (PhysFrame, PhysFrame) {
    let code_frame = frame_allocator::alloc().expect("usermode: no frame for user code");
    let code_phys = code_frame.start_address();
    // SAFETY: `code_phys` is a fresh, identity-mapped frame; the blob is far
    // smaller than a 4 KiB page.
    unsafe {
        core::ptr::copy_nonoverlapping(blob.as_ptr(), code_phys as *mut u8, blob.len());
    }
    paging::map_user_page(UVA_CODE, code_phys, 0).expect("usermode: map user code");

    let stack_frame = frame_allocator::alloc().expect("usermode: no frame for user stack");
    let stack_phys = stack_frame.start_address();
    paging::map_user_page(UVA_STACK, stack_phys, paging::WRITABLE).expect("usermode: map user stack");
    (code_frame, stack_frame)
}

/// Unmap and free a program mapped by [`map_user_program`]. The intermediate
/// page tables leak, exactly like the paging self-tests (reclaiming empty tables
/// is a later refinement) — a bounded, deliberate cost.
fn unmap_user_program(code_frame: PhysFrame, stack_frame: PhysFrame) {
    paging::unmap_page(UVA_CODE);
    paging::unmap_page(UVA_STACK);
    frame_allocator::free(code_frame);
    frame_allocator::free(stack_frame);
}
