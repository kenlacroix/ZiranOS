//! Ziran OS kernel — the Rust half of the system.
//!
//! Everything below the `kernel_main` symbol is ours: no libc, no runtime, no
//! operating system underneath. `#![no_std]` cuts us off from the standard
//! library (which assumes an OS to talk to); we keep only `core`.
//!
//! Control reaches here from hand-written assembly. The boot path is:
//!
//! ```text
//! GRUB (Multiboot2)
//!   -> boot/boot.asm            (32-bit: verify CPU, build page tables, enter long mode)
//!   -> boot/long_mode_init.asm  (64-bit: load segments, call kernel_main)
//!   -> kernel_main              (you are here)
//! ```
//!
//! Milestones realised in this file so far (see STATUS.md):
//!   - M2 "Handing off to Rust": we are running 64-bit Rust code.
//!   - M3 "Making it talk back": VGA text + serial output, no BIOS calls.
#![no_std]
// `core::fmt` machinery is all we need for formatted output.

// Milestone 8: opt into the `alloc` crate (Vec, Box, String). `#![no_std]` does
// not pull it in; the `#[global_allocator]` that backs it lives in `heap`.
extern crate alloc;

use core::panic::PanicInfo;

mod frame_allocator;
mod fs;
mod fw_cfg;
mod gdt;
mod heap;
mod interrupts;
mod keyboard;
mod multiboot;
mod net;
mod paging;
mod pic;
mod pit;
mod port;
mod serial;
mod shell;
mod task;
mod usermode;
mod vga_buffer;

// Bounds of the loaded kernel image, defined by `linker.ld`. These are *symbols*,
// so their addresses — not their (meaningless) byte values — are what we want;
// see the frame allocator, which carves this range out of usable RAM.
extern "C" {
    static kernel_start: u8;
    static kernel_end: u8;
}

/// The kernel entry point, called by `long_mode_init.asm` once the CPU is in
/// 64-bit long mode with a valid stack.
///
/// `extern "C"` gives it the C ABI the assembly expects, and `#[no_mangle]`
/// keeps the symbol name exactly `kernel_main` so the linker can find it.
/// It never returns — there is nowhere to return *to* — so the type is `!`.
///
/// `multiboot_info_addr` is the physical address of the Multiboot2 boot-
/// information structure GRUB handed us. The boot code preserves the pointer
/// GRUB leaves in `EBX` all the way into long mode (see `boot/boot.asm`, where
/// `mov edi, ebx` stashes it), so by the System V AMD64 ABI it arrives here as
/// the first argument in `RDI`. It is a physical address, safe to dereference
/// only because the boot page tables identity-map the first 1 GiB.
#[no_mangle]
pub extern "C" fn kernel_main(multiboot_info_addr: u64) -> ! {
    vga_buffer::clear_screen();

    println!("Ziran OS  --  \u{81ea}\u{7136}");
    println!("self-so: that which arises without external forcing");
    println!();
    println!("[ok] reached 64-bit long mode");
    println!("[ok] Rust kernel_main() running with no OS underneath");
    println!("[ok] VGA text output online (writing straight to 0xb8000)");

    // The serial line is what CI and headless QEMU read; mirror the banner there
    // so an automated boot test has something to assert on.
    serial_println!("Ziran OS booted: kernel_main reached, long mode active.");
    // Milestone 6, step one: prove the Multiboot2 pointer survived the trip from
    // GRUB through the mode-switch trampoline into Rust. This must be a non-null,
    // sub-1-GiB physical address (GRUB places the structure in low memory).
    serial_println!(
        "[ok] Multiboot2 info structure received at {:#018x}",
        multiboot_info_addr
    );

    // Milestone 6: physical memory management. First show the raw memory map GRUB
    // handed us — the ground truth the allocator is built on.
    // SAFETY: `multiboot_info_addr` is GRUB's Multiboot2 pointer, in identity-
    // mapped low memory (see kernel_main's doc comment).
    let boot_info = unsafe { multiboot::BootInfo::new(multiboot_info_addr) };
    match boot_info.memory_map() {
        Some(map) => {
            for region in map {
                serial_println!(
                    "  region base={:#014x} len={:#014x} kind={} {}",
                    region.base,
                    region.length,
                    region.kind,
                    if region.is_usable() { "USABLE" } else { "reserved" }
                );
            }
        }
        None => serial_println!("  (no memory-map tag present)"),
    }

    // Stand up the frame allocator over that map, carving out frame 0, the kernel
    // image, and the Multiboot2 structure. Then prove alloc/free/reclaim work.
    // SAFETY: `kernel_start`/`kernel_end` are linker symbols — we take their
    // addresses, never read them; `multiboot_info_addr` is the valid boot pointer.
    let kernel_start_addr = unsafe { &kernel_start as *const u8 as u64 };
    let kernel_end_addr = unsafe { &kernel_end as *const u8 as u64 };
    unsafe {
        frame_allocator::init(multiboot_info_addr, kernel_start_addr, kernel_end_addr);
    }
    frame_allocator::self_test();
    // Mirror the headline to the screen (the map dump and self-test detail go to
    // serial only, to keep the VGA console readable).
    println!(
        "[ok] physical memory: {} MiB free in the frame allocator",
        frame_allocator::free_frame_count() / 256
    );

    // Milestone 13 (groundwork): replace the two-entry boot GDT with a full one
    // built in Rust — adding ring-3 code/data descriptors and a TSS whose RSP0
    // will catch the ring 3 → ring 0 transition. No ring-3 code runs yet; this
    // just proves the kernel still boots on descriptor tables it owns (the self-
    // test asserts `tr`=0x28). Must precede the IDT: both are descriptor tables,
    // and the syscall gate (later) will reference user privilege levels.
    gdt::init();
    gdt::self_test();

    // Milestone 4: install the interrupt handlers, then prove they work by
    // deliberately triggering a breakpoint. A working IDT catches the `int3`,
    // reports it, and returns here so the kernel keeps running — the difference
    // between "fails gracefully" and "silently reboots".
    interrupts::init();
    println!();
    println!("triggering a test breakpoint (int3)...");
    // SAFETY: `int3` raises #BP, which our IDT handles and returns from.
    unsafe {
        core::arch::asm!("int3");
    }
    println!("[ok] survived the breakpoint -- interrupts work, execution resumed");

    // Milestone 13 (step 3): the syscall gate. Vector 0x80 is now a DPL-3 gate, so
    // ring-3 code will be able to `int 0x80` (step 4). Prove the dispatch works now
    // by issuing it from ring 0 — the same gate accepts CPL 0, and the handler
    // reports CPL=0, confirming the syscall path end to end before any ring-3 code.
    interrupts::syscall_self_test();

    // Milestone 7: build our own page tables and switch CR3 to them. The IDT is
    // already installed (a mapping bug would surface as a reported #PF), and
    // interrupts are still masked, so the switch happens in a quiet moment. If the
    // new tables failed to map this code/stack/GDT/IDT, the machine would triple-
    // fault here instead of printing the marker.
    paging::init();
    paging::self_test();
    // Milestone 13 (step 2): prove user-accessible mappings set U/S=1 at every
    // level of the walk (the leaf alone is not enough — the CPU ANDs U/S down the
    // whole chain). No ring-3 code runs yet; this pins the paging primitive that
    // the userspace excursion will depend on.
    paging::user_map_self_test();
    println!("[ok] paging: switched to kernel-built page tables");

    // Milestone 8: stand up the kernel heap on the new page tables, then prove
    // Vec/Box/String work. Depends on both the frame allocator (M6) and paging
    // (M7); runs while interrupts are still masked.
    heap::init();
    heap::self_test();
    println!("[ok] heap: dynamic allocation (Vec, Box, String) now works");

    // Milestone 9 (cooperative half): stand up the round-robin scheduler on the
    // heap and prove it. Two worker tasks alternate by voluntarily yielding; the
    // self-test asserts their output is a clean "ABAB..." and that each task's
    // registers and heap state survive every switch. Runs while interrupts are
    // still masked — this half needs no timer. See src/task.rs and
    // docs/planning/milestone-09-eng-plan.md.
    task::cooperative_self_test();
    println!("[ok] scheduler: two tasks cooperatively multitask (Milestone 9)");

    // Milestone 5: bring up the keyboard. Remap the PIC first, THEN enable
    // hardware interrupts — doing it in the other order could let a stray IRQ
    // fire into an exception vector before the PIC is configured. `pic::init`
    // now unmasks both the keyboard (IRQ1) and the timer (IRQ0, Milestone 9b).
    pic::init();
    serial_println!("Ziran OS: PIC remapped, timer + keyboard IRQs unmasked.");

    // Milestone 9b: program the periodic timer BEFORE the `sti` below. IRQ0 was
    // just unmasked at the PIC, but interrupts are still disabled (IF=0), so any
    // tick from the PIT's power-on 18.2 Hz default merely latches pending in the
    // PIC and is not delivered. By the time `sti` lets it through, the divisor is
    // set, so the first IRQ0 we actually take is at our chosen 100 Hz.
    pit::init(100);

    println!();
    println!("keyboard is live -- type something:");
    println!();

    // SAFETY: the IDT (M4), PIC, and PIT are configured; it is now safe to let
    // hardware interrupts through. `sti` sets the interrupt flag — from here the
    // timer fires 100×/second and drives preemption.
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }

    // Milestone 9b: prove preemption. Two tasks that NEVER yield are interleaved
    // purely by the timer interrupt — if that failed, this would hang rather than
    // return. See src/task.rs.
    task::preemptive_self_test();
    println!("[ok] scheduler: preemptive multitasking works (Milestone 9)");

    // Milestone 10: hand the machine to an interactive shell. First prove the
    // line editor deterministically (interactive typing is a manual check; the
    // editing logic is pinned by this self-test). The self-tests above each
    // rebuilt the scheduler and left it full of finished tasks, so start a fresh
    // one, then spawn the shell as a task and yield into it. `kernel_main` becomes
    // the idle task (id 0): it falls into hlt_loop below, woken by each tick so
    // the scheduler can preempt back to the shell. See src/shell.rs.
    // Milestone 11: stand up the read-only RAM-disk filesystem and prove it (mount,
    // list, exact bytes, and four corrupt-image rejections) before the shell — which
    // gains `ls`/`cat` — comes up. Pure heap-backed data, no interrupt state.
    fs::self_test();

    shell::self_test();

    // Milestone 14: the networking stub. A single in-kernel loopback interface with
    // a minimal socket layer (bind/send/poll/recv) — a socket is a named endpoint
    // with a receive queue, reached by address. No IP/TCP/ports/device/wire (the
    // stretch-stub carve-out of the networking non-goal). See net.rs.
    net::init();
    net::self_test();

    // Milestone 13 (step 4): the milestone payload. Run one program in ring 3,
    // have it print 'Z' via the `int 0x80` syscall gate from CPL 3, and return to
    // the kernel. This is the first time code runs that the *hardware* holds back.
    // Runs inline here (not as a scheduled task — folding ring 3 into the preemptive
    // scheduler is a deferred M13 cut) and guards itself against the timer.
    usermode::self_test();

    // Milestone 13 (step 5): the enforcement test — the M15 preview. Two ring-3
    // blobs deliberately violate the boundary (run `cli`, read a kernel page); the
    // CPU faults (#GP / #PF) and the handler catches each from CPL 3 and unwinds.
    // This is the proof the boundary is real, not just unused.
    usermode::enforcement_test();

    // Milestone 15: break the privilege boundary. Plant a FLAG on a kernel-only
    // page and attack it from ring 3 — a direct read (CPU faults it) and the
    // confused-deputy syscall (leaks it without a copy-from-user check, holds with
    // one). The lesson beyond M13: the CPU stops unauthorized *access*, but only
    // software stops authorized *misuse*. See docs/planning/milestone-15-eng-plan.md.
    usermode::security_test();

    // Tier-2 CTF mode: if the CTF host injected a per-session secret via
    // `-fw_cfg opt/flag`, install it so `load` runs the remote-capture path (append
    // the flag past the player's bytes, mount with the length-trusting reader). Plain
    // boots with no such entry — e.g. the in-browser Tier-1 kernel — skip this and
    // stay in strict mode. Needs the heap (M8), which is up by now.
    if let Some(flag) = fw_cfg::read_flag() {
        let n = flag.len();
        shell::set_tier2_flag(flag);
        serial_println!("[m16] tier-2: fw_cfg opt/flag present ({n} bytes) -- remote-capture mode");
    }

    task::init();
    task::spawn(shell::shell_main);
    task::yield_now();

    // Idle. The CPU halts until an interrupt (a timer tick or keystroke) wakes it;
    // the scheduler runs the shell, and control returns here to halt again.
    hlt_loop();
}

/// Halt the CPU until the next interrupt, forever. `hlt` in a loop keeps a
/// finished kernel from spinning a core at 100%. Once interrupts are wired up
/// (Milestone 4) an interrupt will wake it and it will simply halt again.
pub fn hlt_loop() -> ! {
    loop {
        // SAFETY: `hlt` only halts the current CPU until the next interrupt;
        // it has no memory effects and is always valid in ring 0.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// The panic handler. In a kernel there is no process to kill and no one to
/// unwind to, so we report what happened over both output channels and wedge
/// the machine on a halt loop rather than corrupting more state.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Serial first: it is the channel a headless run can actually capture.
    serial_println!("KERNEL PANIC: {}", info);
    println!("\nKERNEL PANIC: {}", info);
    hlt_loop();
}
