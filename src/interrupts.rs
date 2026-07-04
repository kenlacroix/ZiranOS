//! Interrupts and exceptions — Milestone 4, "Teaching it to fail gracefully."
//!
//! Until now a mistake in the kernel — a divide by zero, a bad memory access —
//! would make the CPU escalate through fault → double fault → *triple* fault,
//! at which point the machine just resets. No message, no clue. This module
//! installs an **Interrupt Descriptor Table (IDT)**: a 256-entry lookup that
//! tells the CPU, "when interrupt/exception number N happens, jump *here*."
//! With handlers in place, a fault becomes a printed report instead of a silent
//! reboot — and some faults (like a debug breakpoint) can be handled and
//! execution resumed as if nothing happened.
//!
//! See `docs/concepts/interrupts.md` for the from-first-principles explanation
//! of everything referenced here (GDT, IDT, gates, the calling convention).
//!
//! The per-vector entry stubs live in `boot/isr.asm`; this file is the Rust half
//! that builds the table and decides what each fault *means*.

use crate::{hlt_loop, keyboard, pic, print, println, serial_print, serial_println};
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

/// The vector the timer's IRQ0 is remapped to (see `pic`): 0x20 + 0 = 0x20.
const TIMER_VECTOR: usize = pic::PIC1_OFFSET as usize;
/// The vector the keyboard's IRQ1 is remapped to (see `pic`): 0x20 + 1 = 0x21.
const KEYBOARD_VECTOR: usize = pic::PIC1_OFFSET as usize + 1;

/// Milestone 13: the software-interrupt vector ring-3 code uses to call the
/// kernel — the classic `int 0x80`. Its gate is installed at DPL 3 (see
/// [`IdtEntry::set_user_handler`]) so ring 3 may invoke it.
const SYSCALL_VECTOR: usize = 0x80;

/// Syscall numbers, passed by the caller in `rax`. The minimal M13 set: print one
/// character (`rdi` = the byte) and signal the user excursion is finished. M15
/// adds the first *pointer-carrying* syscalls (`rdi` = buffer, `rsi` = length).
pub const SYS_PRINT: u64 = 1;
pub const SYS_EXIT: u64 = 2;
/// M15: write `rsi` bytes from the user buffer at `rdi` to the console — the
/// kernel's first copy-from-user. Validates the range with [`crate::paging::user_range_ok`]
/// before touching it, so a caller cannot make the kernel read memory the caller
/// couldn't. Returns the byte count, or -1 (`u64::MAX`) on rejection.
pub const SYS_WRITE: u64 = 3;
/// M15, the teaching device: `SYS_WRITE` *with the copy-from-user check omitted* —
/// the confused deputy. It dereferences whatever pointer the caller passes with
/// full ring-0 power, so a ring-3 program can make it read a kernel-only page (the
/// planted FLAG) and hand the bytes back. Worse still, an unmapped or non-canonical
/// pointer would fault the *kernel* (a ring-0 #PF/#GP the ring-3 recovery path
/// doesn't handle → fatal), so the missing check is both an info-leak and a crash
/// primitive. Deliberately broken to be *observed* (like M11's loose extent
/// check); it must never exist in a hardened kernel. See
/// `docs/planning/milestone-15-eng-plan.md`.
pub const SYS_WRITE_UNCHECKED: u64 = 4;

/// Upper bound on a single `SYS_WRITE`. A syscall must never do an unbounded copy
/// on a caller-supplied length — a huge `rsi` is itself an attack (DoS, and the
/// arithmetic surface the red-team named). Both write arms reject `len > MAX_WRITE`
/// *before* any dereference or validation.
const MAX_WRITE: usize = 256;

/// Capture of the bytes the most recent `SYS_WRITE`/`SYS_WRITE_UNCHECKED` emitted,
/// so the M15 self-test can prove a *real* flag capture by byte-equality against
/// the planted secret — not by trusting whatever scrolled past on the console
/// (office-hours Q4: working vs. accidentally working). Plain atomics: the write
/// arm runs with IF=0 inside the single-threaded ring-3 excursion, and the test
/// reads it only after the excursion returns.
static LAST_WRITE_BUF: [AtomicU8; MAX_WRITE] = [const { AtomicU8::new(0) }; MAX_WRITE];
static LAST_WRITE_LEN: AtomicUsize = AtomicUsize::new(0);

/// Clear the capture buffer before an excursion, so a stale value from a prior
/// write can't masquerade as a fresh capture. Zeroes the bytes too, not just the
/// length: in a module whose whole point is "the flag must not leak," letting the
/// captured secret sit resident in a static after the test read it is a smell —
/// so the "nothing leaked" case is structurally empty, not merely unread.
pub fn reset_last_write() {
    for slot in LAST_WRITE_BUF.iter() {
        slot.store(0, Ordering::SeqCst);
    }
    LAST_WRITE_LEN.store(0, Ordering::SeqCst);
}

/// Copy the bytes the most recent write emitted into `out`; returns the length
/// (clamped to `out`). Read by the M15 self-test to assert a capture.
pub fn last_write_into(out: &mut [u8]) -> usize {
    let n = LAST_WRITE_LEN.load(Ordering::SeqCst).min(out.len());
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        *slot = LAST_WRITE_BUF[i].load(Ordering::SeqCst);
    }
    n
}

/// The exact register state our assembly stub (`isr_common` in boot/isr.asm)
/// leaves on the stack, in ascending memory order. `interrupt_dispatch` receives
/// a pointer to this. The field order MUST match the push order in the assembly
/// exactly — a mismatch would silently misread every field.
///
/// `repr(C)` keeps Rust from reordering the fields.
#[repr(C)]
#[derive(Debug)]
pub struct InterruptContext {
    // Pushed by isr_common, restored in reverse. r15 was pushed last, so it sits
    // at the lowest address — hence first here.
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    // Pushed by the per-vector stub.
    pub vector: u64,
    pub error_code: u64,
    // Pushed by the CPU itself, automatically, when the interrupt was taken.
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// A single 16-byte IDT gate descriptor. The handler's 64-bit address is,
/// bizarrely, split across three non-adjacent fields — a backward-compatibility
/// scar from the 16- and 32-bit eras. We reassemble it in `set_handler`.
#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,   // bits 0..16 of the handler address
    selector: u16,     // code segment selector the handler runs under
    ist: u8,           // Interrupt Stack Table index (0 = use the current stack)
    type_attr: u8,     // gate type + privilege + present bit
    offset_mid: u16,   // bits 16..32 of the handler address
    offset_high: u32,  // bits 32..64 of the handler address
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// Point this gate at `handler`, running under code segment `selector`, with
    /// the given `type_attr` (present bit | DPL | gate type). The 64-bit handler
    /// address is split across the three offset fields.
    fn set_gate(&mut self, handler: u64, selector: u16, type_attr: u8) {
        self.offset_low = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.selector = selector;
        self.ist = 0;
        self.type_attr = type_attr;
        self.reserved = 0;
    }

    /// The default: a ring-0-only interrupt gate.
    /// 0x8E = present (0x80) | DPL 0 | type 0xE (64-bit *interrupt* gate). An
    /// interrupt gate clears the interrupt flag on entry, so a handler is not
    /// itself interrupted before it is ready — the safe default.
    fn set_handler(&mut self, handler: u64, selector: u16) {
        self.set_gate(handler, selector, 0x8E);
    }

    /// Milestone 13: a gate ring-3 code may invoke with `int` — the syscall gate.
    /// 0xEE = present | **DPL 3** | 64-bit interrupt gate. The DPL 3 is required:
    /// the CPU checks `CPL <= gate.DPL` on a software `int`, so an `int 0x80` from
    /// ring 3 through a DPL-0 gate raises #GP instead of entering the handler.
    /// Still an *interrupt* gate (IF cleared on entry), so the syscall handler is
    /// not itself preemptible.
    fn set_user_handler(&mut self, handler: u64, selector: u16) {
        self.set_gate(handler, selector, 0xEE);
    }
}

/// The table itself: 256 gates, plus the CPU expects it 16-byte aligned.
#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; 256],
}

/// The value handed to the `lidt` instruction: the table's size (minus one, a
/// hardware quirk) and its base address. Packed so the two fields are adjacent
/// with no padding, exactly as the CPU reads them.
#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

/// Our single, static IDT. It lives for the whole life of the kernel, which is
/// important: `lidt` only stores a *pointer* to it, so the memory must never
/// move or be freed.
static mut IDT: Idt = Idt {
    entries: [IdtEntry::missing(); 256],
};

extern "C" {
    /// Filled in by `boot/isr.asm`: 256 entry-stub addresses, one per vector.
    static isr_stub_table: [u64; 256];
}

/// The code segment selector set up by the boot GDT (`boot/boot.asm`): the null
/// descriptor is entry 0, so our 64-bit code segment is at byte offset 8.
const KERNEL_CODE_SELECTOR: u16 = 0x08;

/// Build the IDT — wiring every vector to its assembly stub — and load it.
///
/// Safe to call once, early, before any interrupt could fire.
pub fn init() {
    // SAFETY: single-threaded, interrupts still disabled; we are the only writer
    // of IDT and we read the stub table (immutable) that the linker resolved.
    unsafe {
        let idt = &mut *core::ptr::addr_of_mut!(IDT);
        for vector in 0..256 {
            idt.entries[vector].set_handler(isr_stub_table[vector], KERNEL_CODE_SELECTOR);
        }
        // Milestone 13: re-arm the syscall vector as a DPL-3 gate so ring-3 code
        // can reach it. Same stub as every other vector (isr_common saves the full
        // context and iretqs); only the gate privilege differs. Not reachable from
        // ring 3 until the iretq launch (step 4), but a ring-0 `int 0x80` already
        // exercises the dispatch (see `syscall_self_test`).
        idt.entries[SYSCALL_VECTOR].set_user_handler(isr_stub_table[SYSCALL_VECTOR], KERNEL_CODE_SELECTOR);

        let pointer = IdtPointer {
            limit: (core::mem::size_of::<Idt>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }

    serial_println!("[ok] IDT loaded: 256 vectors wired to handlers");
}

/// Human-readable names for the CPU's first 32 (architecture-defined) vectors.
/// Everything at 32+ is a hardware/software interrupt we haven't assigned yet.
const EXCEPTIONS: [&str; 32] = [
    "#DE Divide Error",
    "#DB Debug",
    "NMI Non-Maskable Interrupt",
    "#BP Breakpoint",
    "#OF Overflow",
    "#BR BOUND Range Exceeded",
    "#UD Invalid Opcode",
    "#NM Device Not Available",
    "#DF Double Fault",
    "Coprocessor Segment Overrun",
    "#TS Invalid TSS",
    "#NP Segment Not Present",
    "#SS Stack-Segment Fault",
    "#GP General Protection Fault",
    "#PF Page Fault",
    "reserved",
    "#MF x87 Floating-Point Error",
    "#AC Alignment Check",
    "#MC Machine Check",
    "#XM SIMD Floating-Point Exception",
    "#VE Virtualization Exception",
    "#CP Control Protection Exception",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
    "reserved",
];

/// Called from assembly (`isr_common`) for every interrupt and exception, with a
/// pointer to the saved machine state. This is where a fault stops being a raw
/// CPU event and becomes something the kernel reasons about.
///
/// `#[no_mangle] extern "C"` so the assembly can find it by name and call it
/// with the C ABI it uses.
/// Called from assembly for every trap. NOTE (known limitation): this prints
/// through the same spinlock-guarded VGA/serial writers as normal code. If a
/// fault ever fires *while* `kernel_main` holds that lock (mid-print), the
/// handler would spin forever waiting for a lock only it could release. In M4
/// the only trap we raise (`int3`) happens at a point where no writer lock is
/// held, so this is safe today; a lock-free emergency writer is the proper fix
/// once faults can occur at arbitrary points.
#[no_mangle]
pub extern "C" fn interrupt_dispatch(ctx: &mut InterruptContext) {
    let vector = ctx.vector as usize;

    match vector {
        // #BP Breakpoint (int3): the friendly one. We report it and simply
        // return — `iretq` resumes the instruction after the `int3`. This is the
        // proof that the whole path works AND that recovery is possible.
        3 => {
            let rip = ctx.rip;
            println!("[trap] breakpoint (#BP) at {:#018x} -- handled, resuming", rip);
            serial_println!("[trap] #BP at {:#018x} handled", rip);
        }

        // Milestone 13 — the enforcement gate (the M15 privilege-boundary preview).
        // A #GP or #PF taken *from ring 3* (saved CS shows CPL 3) is a deliberate
        // boundary violation: ring 3 ran a privileged instruction (`cli` → #GP) or
        // touched a kernel-only page (`mov rax,[0xb8000]` → #PF, U/S bit set). The
        // CPU stopping it *is* the boundary working. Report it — with the faulting
        // CS proving CPL 3 — then unwind the excursion back to ring 0 rather than
        // halting the kernel (a ring-0 fault below still halts, being a real bug).
        // Scope: only #GP/#PF are contained (all the M13 blobs raise). Any *other*
        // ring-3 fault (#UD, #DE, …) falls through to the halt arm below — fine for
        // this cut, but broadening ring-3 containment is future work once real user
        // programs run.
        v @ (13 | 14) if ctx.cs & 3 == 3 => {
            let name = EXCEPTIONS.get(v).copied().unwrap_or("exception");
            println!(
                "[m13] blocked ring-3 violation: {} at RIP={:#018x} CS={:#x} (CPL={})",
                name,
                ctx.rip,
                ctx.cs,
                ctx.cs & 3
            );
            serial_println!(
                "[m13] blocked ring-3 violation: {} at RIP={:#018x} CS={:#x} (CPL={}) err={:#x}",
                name,
                ctx.rip,
                ctx.cs,
                ctx.cs & 3,
                ctx.error_code
            );
            if v == 14 {
                // #PF: CR2 is the address ring 3 tried to touch; the error-code
                // U/S bit (bit 2) set proves the CPU denied a *user-mode* access.
                let cr2 = read_cr2();
                let user_access = ctx.error_code & 0b100 != 0;
                serial_println!(
                    "  -> tried to touch {:#018x}: {} (U/S bit set: {})",
                    cr2,
                    describe_page_fault(ctx.error_code),
                    user_access
                );
            }
            crate::usermode::recover_from_violation(ctx, v as u64);
        }

        // #PF Page Fault: the address that faulted is in CR2, and the error code
        // is a bitfield describing the access. We can't recover yet (paging
        // management is Milestones 6-7), so we report richly and halt.
        14 => {
            let cr2 = read_cr2();
            fatal_header(ctx);
            println!("  faulting address (CR2): {:#018x}", cr2);
            println!("  cause: {}", describe_page_fault(ctx.error_code));
            halt();
        }

        // Timer IRQ0 (Milestone 9b). Count the tick, acknowledge the PIC, then
        // preempt: hand the CPU to the next runnable task. The order matters —
        // the EOI MUST precede the switch, or the outgoing task's tick is never
        // acknowledged and the PIC delivers no more timer interrupts (the clock
        // appears to hang after one fire). The switch itself reuses the same
        // `switch_context` a cooperative `yield_now` uses: because we run here
        // with IF cleared (interrupt gate) and the scheduler lock is
        // interrupt-safe, this cannot deadlock. This arm MUST stay above the
        // spurious-range guard below, or IRQ0 would be swallowed as spurious.
        TIMER_VECTOR => {
            crate::pit::handle_interrupt();
            pic::send_eoi(0);
            crate::task::preempt();
        }

        // Keyboard IRQ (Milestone 5, rewired for Milestone 10). The handler is now
        // a pure *producer*: decode the scancode and drop the byte into the
        // lock-free input ring, then EOI and return. Echoing and all handling move
        // to the shell *task* (the consumer). This is deliberate — it also removes
        // a latent deadlock: the handler no longer takes the VGA writer lock, so it
        // can never spin on a lock a preempted task is holding mid-print.
        KEYBOARD_VECTOR => {
            if let Some(c) = keyboard::handle_interrupt() {
                // `translate` only ever yields ASCII, so this cast never truncates;
                // assert it so a future non-ASCII mapping fails loudly, not silently.
                debug_assert!(c.is_ascii(), "keyboard: non-ASCII char would truncate in the ring");
                keyboard::push(c as u8);
            }
            pic::send_eoi(1);
        }

        // Milestone 13: the syscall gate. A software `int 0x80` lands here — from
        // ring 0 in the step-3 self-test, and from ring 3 once the launch exists.
        // No PIC EOI: this is a software interrupt, not a hardware IRQ.
        SYSCALL_VECTOR => syscall(ctx),

        // Any other vector in the PIC's range (0x20..0x2F) that we didn't
        // unmask: almost certainly a *spurious* interrupt, which real PICs emit
        // on IRQ7/IRQ15 when a line glitches. Do NOT halt over one, and — key
        // detail — do NOT send an EOI for a spurious IRQ, so we send none here.
        v if (pic::PIC1_OFFSET as usize..pic::PIC1_OFFSET as usize + 16).contains(&v) => {}

        // Everything else: report and halt. In the exception range this is a
        // real bug the kernel should surface, not hide. Halting legibly is the
        // goal ("fail gracefully", not "fail invisibly").
        _ => {
            fatal_header(ctx);
            halt();
        }
    }
}

/// Service a `int 0x80` syscall. The number is in `rax`, arguments in
/// `rdi`/`rsi`/`rdx`; the return value is written back into `rax`, which
/// `isr_common` restores on `iretq` (it saved the context from this same stack
/// slot). The saved `CS` reveals the caller's privilege — `cs & 3` is the CPL,
/// which is **3** when the call genuinely came from ring 3. That check is the
/// M13 "working vs. accidentally working" gate: the caller's self-test asserts
/// the CPL, so a syscall accidentally serviced from ring 0 can't masquerade as a
/// ring-3 crossing.
fn syscall(ctx: &mut InterruptContext) {
    let number = ctx.rax;
    let cpl = ctx.cs & 3;
    match number {
        SYS_PRINT => {
            // One character, passed by value in rdi — no user pointer to validate
            // yet (that, and copy-from-user, is the deferred M15 "confused deputy"
            // surface). Print it to VGA (the visible demo) and serial (CI).
            let ch = ctx.rdi as u8 as char;
            print!("{}", ch);
            serial_println!("[m13] syscall: SYS_PRINT {:?} (CS={:#x}, CPL={})", ch, ctx.cs, cpl);
            ctx.rax = 0; // success
        }
        SYS_EXIT => {
            // The user excursion is finished. Hand back to the kernel: rewrite this
            // interrupt's saved frame so the ISR's iretq returns to ring 0 (see
            // `usermode::resume_kernel`) rather than back to the ring-3 blob. This
            // does not return here — the frame now points at the kernel.
            serial_println!("[m13] syscall: SYS_EXIT (CS={:#x}, CPL={})", ctx.cs, cpl);
            crate::usermode::resume_kernel(ctx);
        }
        SYS_WRITE => sys_write(ctx, true),
        SYS_WRITE_UNCHECKED => sys_write(ctx, false),
        _ => {
            serial_println!("[m13] syscall: unknown number {} (CPL={})", number, cpl);
            ctx.rax = u64::MAX; // -1: unknown syscall
        }
    }
}

/// The M15 copy-from-user path shared by `SYS_WRITE` (`validate = true`) and
/// `SYS_WRITE_UNCHECKED` (`validate = false`). Reads `ctx.rsi` bytes from the user
/// pointer in `ctx.rdi`, prints them, and records them for the self-test; sets
/// `ctx.rax` to the byte count on success or `u64::MAX` (-1) on rejection.
///
/// This is the whole M15 lesson in one function. The kernel is a *deputy* acting
/// on the caller's behalf with ring-0 power. Ring 3 cannot read a kernel page
/// directly — the CPU faults it (M13). But if the kernel dereferences a pointer
/// the caller chose, the CPU sees a ring-0 access and allows it: the boundary is
/// silent. `validate` is the software check that closes that gap by proving,
/// *without dereferencing*, that the caller could have read the range itself. Drop
/// it and the deputy is confused — it reads the kernel-only FLAG and hands it back.
fn sys_write(ctx: &mut InterruptContext, validate: bool) {
    let ptr = ctx.rdi;
    let len = ctx.rsi;
    let cpl = ctx.cs & 3;

    // Bound the length first — before any validation or dereference. An unbounded
    // copy on a caller-chosen length is an attack regardless of the pointer.
    if len as usize > MAX_WRITE {
        serial_println!(
            "[m15] SYS_WRITE rejected: len {} exceeds MAX_WRITE {} (CPL={})",
            len, MAX_WRITE, cpl
        );
        ctx.rax = u64::MAX;
        return;
    }

    // The confused-deputy check. `user_range_ok` walks the page tables (never the
    // memory) and confirms every page of [ptr, ptr+len) is present and U/S=1 — i.e.
    // the caller could have read it unaided. Skipped by SYS_WRITE_UNCHECKED, which
    // is exactly how the kernel-only FLAG leaks.
    if validate && !crate::paging::user_range_ok(ptr, len) {
        serial_println!(
            "[m15] SYS_WRITE rejected by copy_from_user: [{:#x}, +{}) is not user-readable (CPL={})",
            ptr, len, cpl
        );
        ctx.rax = u64::MAX;
        return;
    }

    let n = len as usize;
    LAST_WRITE_LEN.store(0, Ordering::SeqCst);
    serial_print!(
        "[m15] SYS_WRITE{} emitted {} bytes: \"",
        if validate { "" } else { "_UNCHECKED" },
        n
    );
    for i in 0..n {
        // SAFETY (validate = true): `user_range_ok` just proved every page of
        // [ptr, ptr+len) present *and* the whole range canonical (it rejects any
        // end past 2^47), so this read can raise neither #PF (present) nor #GP
        // (canonical) — it cannot fault.
        // SAFETY (validate = false): there is NONE — `ptr` is attacker-chosen and
        // may name a kernel page (leaks it) or be unmapped/non-canonical (takes a
        // ring-0 #PF/#GP the ring-3 recovery path won't catch → fatal). This is the
        // deliberate confused-deputy footgun the milestone demonstrates; bounded
        // `n` is the only limit.
        let byte = unsafe { core::ptr::read_volatile((ptr as *const u8).add(i)) };
        LAST_WRITE_BUF[i].store(byte, Ordering::SeqCst);
        print!("{}", byte as char); // the visible demo on VGA
        serial_print!("{}", byte as char); // and on the serial log CI reads
    }
    serial_println!("\" (CPL={})", cpl);
    LAST_WRITE_LEN.store(n, Ordering::SeqCst);
    ctx.rax = len; // bytes written
}

/// Milestone 13 (step 3): prove the syscall path end-to-end *before* ring 3
/// exists, by issuing `int 0x80` from ring 0. The DPL-3 gate accepts it (CPL 0 ≤
/// gate DPL 3), the dispatcher services `SYS_PRINT`, and the result returns in
/// `rax`. From ring 0 the handler reports `CPL=0` — which is exactly right, and
/// confirms the CPL readout is live: step 4 issues the same call from ring 3 and
/// must instead see `CPL=3`. Runs with interrupts still masked, so nothing holds
/// the print lock the handler takes — no deadlock window.
pub fn syscall_self_test() {
    let ret: u64;
    // SAFETY: `int 0x80` traps into our own DPL-3 gate; `isr_common` saves and
    // restores every register, so only rax (in: number, out: result) and rdi (in:
    // the char) participate. No memory the compiler tracks is clobbered beyond the
    // print, which the default (non-`nomem`) options already assume.
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inout("rax") SYS_PRINT => ret,
            in("rdi") b'?' as u64,
        );
    }
    assert_eq!(ret, 0, "SYS_PRINT should return 0, got {:#x}", ret);
    serial_println!(
        "[ok] M13: int 0x80 syscall gate wired — SYS_PRINT dispatched from ring 0 (CPL=0), returned {}",
        ret
    );
}

/// Are hardware interrupts currently enabled (RFLAGS.IF set)?
pub fn are_enabled() -> bool {
    let flags: u64;
    // SAFETY: `pushfq; pop` only reads RFLAGS into a register; it modifies no
    // memory Rust cares about and leaves the flags unchanged.
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    flags & (1 << 9) != 0
}

/// Enable hardware interrupts (`sti`). Valid once the IDT and PIC are configured.
pub fn enable() {
    // SAFETY: `sti` only sets RFLAGS.IF; always valid in ring 0.
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

/// Disable interrupts, returning whether they had been enabled — the "save" half
/// of a critical-section guard. Pair with [`restore`].
pub fn save_and_disable() -> bool {
    let was = are_enabled();
    // SAFETY: `cli` only clears RFLAGS.IF; always valid in ring 0.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    was
}

/// Restore interrupts to a previously-saved state: re-enable only if they were
/// enabled when [`save_and_disable`] ran. Never turns on interrupts that were off.
pub fn restore(was_enabled: bool) {
    if was_enabled {
        enable();
    }
}

/// Print the common header for a fatal fault: which one, where, and the error
/// code if the CPU supplied a meaningful one.
fn fatal_header(ctx: &InterruptContext) {
    let vector = ctx.vector as usize;
    let name = EXCEPTIONS.get(vector).copied().unwrap_or("interrupt");
    println!();
    println!("*** UNHANDLED {} (vector {}) ***", name, vector);
    println!("  at RIP {:#018x}  CS {:#x}", ctx.rip, ctx.cs);
    if ctx.error_code != 0 {
        println!("  error code: {:#x}", ctx.error_code);
    }
    serial_println!(
        "PANIC: {} (vec {}) at {:#018x}, err {:#x}",
        name,
        vector,
        ctx.rip,
        ctx.error_code
    );
}

/// Decode the page-fault error-code bitfield into words. These bits are defined
/// by the architecture; see docs/concepts/interrupts.md.
fn describe_page_fault(code: u64) -> &'static str {
    let present = code & 1 != 0; // 0 = page not present, 1 = protection violation
    let write = code & 2 != 0; // the access was a write
    match (present, write) {
        (false, false) => "read from a non-present page",
        (false, true) => "write to a non-present page",
        (true, false) => "read that violated page protection",
        (true, true) => "write that violated page protection",
    }
}

fn halt() -> ! {
    println!("  halted. (attach GDB with `make gdb`, or check the -d int log)");
    hlt_loop();
}

/// Read CR2, which the CPU loads with the faulting linear address on a #PF.
fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: reading a control register has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}
