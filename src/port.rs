//! x86 port-mapped I/O — the `in`/`out` instructions.
//!
//! Alongside memory, x86 has a second, separate address space of 65,536 "I/O
//! ports" reached only by the `in`/`out` instructions. Legacy hardware — the
//! interrupt controller, the PS/2 keyboard, the serial UART, the PIT timer —
//! lives here rather than in memory. These two functions are the whole
//! interface; every device driver in the kernel is built on them.

/// Write a byte to an I/O port.
///
/// SAFETY: talking to a hardware port can have arbitrary side effects (it may
/// reconfigure a device). The caller must know the port and value are correct
/// for the device being programmed.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") value,
        options(nomem, nostack, preserves_flags),
    );
}

/// Read a byte from an I/O port.
///
/// SAFETY: reading a port can also have side effects (e.g. reading the keyboard
/// data port advances its buffer). Caller must target a valid device register.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags),
    );
    value
}

/// Write a 16-bit word to an I/O port. Needed for registers wider than a byte —
/// e.g. the QEMU `fw_cfg` selector at port 0x510 (see `fw_cfg`).
///
/// SAFETY: as [`outb`] — the caller must know the port and value are correct for
/// the device being programmed.
#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") value,
        options(nomem, nostack, preserves_flags),
    );
}
