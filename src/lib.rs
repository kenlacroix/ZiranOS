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

use core::panic::PanicInfo;

mod interrupts;
mod keyboard;
mod pic;
mod port;
mod serial;
mod vga_buffer;

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

    // Milestone 5: bring up the keyboard. Remap + mask the PIC first, THEN
    // enable hardware interrupts — doing it in the other order could let a stray
    // IRQ fire into an exception vector before the PIC is configured.
    pic::init();
    serial_println!("Ziran OS: PIC remapped, keyboard IRQ unmasked.");
    println!();
    println!("keyboard is live -- type something:");
    println!();

    // SAFETY: the IDT (M4) and PIC are configured; it is now safe to let
    // hardware interrupts through. `sti` sets the interrupt flag.
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }

    // Idle. The CPU halts until an interrupt (a keystroke) wakes it, the handler
    // echoes the character, and `iretq` returns us right back here to halt again.
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
