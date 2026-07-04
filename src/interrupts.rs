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

use crate::{hlt_loop, keyboard, pic, print, println, serial_println};

/// The vector the timer's IRQ0 is remapped to (see `pic`): 0x20 + 0 = 0x20.
const TIMER_VECTOR: usize = pic::PIC1_OFFSET as usize;
/// The vector the keyboard's IRQ1 is remapped to (see `pic`): 0x20 + 1 = 0x21.
const KEYBOARD_VECTOR: usize = pic::PIC1_OFFSET as usize + 1;

/// Milestone 13: the software-interrupt vector ring-3 code uses to call the
/// kernel — the classic `int 0x80`. Its gate is installed at DPL 3 (see
/// [`IdtEntry::set_user_handler`]) so ring 3 may invoke it.
const SYSCALL_VECTOR: usize = 0x80;

/// Syscall numbers, passed by the caller in `rax`. The minimal M13 set: print one
/// character (`rdi` = the byte) and signal the user excursion is finished.
pub const SYS_PRINT: u64 = 1;
pub const SYS_EXIT: u64 = 2;

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
            // The user excursion is finished. The minimal cut only reports it here;
            // actually *unwinding* to ring 0 (rather than iretq-ing back to the
            // blob) is wired with the launch in the next step.
            serial_println!("[m13] syscall: SYS_EXIT (CS={:#x}, CPL={})", ctx.cs, cpl);
            ctx.rax = 0;
        }
        _ => {
            serial_println!("[m13] syscall: unknown number {} (CPL={})", number, cpl);
            ctx.rax = u64::MAX; // -1: unknown syscall
        }
    }
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
