//! The 8259 Programmable Interrupt Controller (PIC) — Milestone 5.
//!
//! Hardware devices don't interrupt the CPU directly; they raise a line on the
//! PIC, which decides what reaches the CPU and in what priority. A PC has two
//! 8259s chained together (a "master" and a "slave"), giving 15 interrupt lines
//! (IRQs). We must do two things before enabling interrupts:
//!
//!   1. **Remap** the IRQs. Out of reset the PICs deliver IRQ0..15 as CPU
//!      vectors 0x08..0x0F and 0x70..0x77. But vectors 0..31 are reserved by the
//!      CPU for exceptions — so IRQ0 (the timer) would masquerade as a #DF! We
//!      remap the two PICs to vectors 0x20..0x2F, safely above the exceptions.
//!   2. **Mask** every line we're not ready for. For this milestone only the
//!      keyboard (IRQ1) is unmasked; the timer and the rest stay silent until
//!      later milestones want them.
//!
//! See docs/concepts/keyboard-and-pic.md for the full picture.

use crate::port::outb;

// Command and data ports for the master and slave PICs.
const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;

/// The vector the master PIC's IRQ0 is remapped to. IRQ n arrives as
/// `PIC1_OFFSET + n`, so the keyboard (IRQ1) becomes vector 0x21.
pub const PIC1_OFFSET: u8 = 0x20;
pub const PIC2_OFFSET: u8 = 0x28;

const CMD_INIT: u8 = 0x11; // ICW1: begin init, expect ICW4
const MODE_8086: u8 = 0x01; // ICW4: 8086/88 mode
const CMD_EOI: u8 = 0x20; // end-of-interrupt

/// Give an old PIC a moment to latch a command. Real 8259s can need a short gap
/// between writes; the conventional trick is a throwaway write to the unused
/// POST port 0x80, which takes ~1µs of bus time. QEMU doesn't need it, but real
/// hardware sometimes does — cheap insurance.
#[inline]
unsafe fn io_wait() {
    outb(0x80, 0);
}

/// Remap both PICs and mask all lines except the keyboard (IRQ1).
///
/// Must be called before `sti`. The init sequence writes four "initialisation
/// command words" (ICW1..4) to each PIC in a fixed order the chip expects.
pub fn init() {
    // SAFETY: this is the architecturally-defined 8259 init handshake; every
    // port/value is fixed by the hardware. No interrupts are enabled yet.
    unsafe {
        // ICW1: start the init sequence on both chips.
        outb(PIC1_COMMAND, CMD_INIT);
        io_wait();
        outb(PIC2_COMMAND, CMD_INIT);
        io_wait();

        // ICW2: the vector offset each PIC's IRQs are remapped to.
        outb(PIC1_DATA, PIC1_OFFSET);
        io_wait();
        outb(PIC2_DATA, PIC2_OFFSET);
        io_wait();

        // ICW3: describe the master/slave wiring. The slave is cascaded on the
        // master's IRQ2, so tell the master "line 2 has a slave" (bitmask 1<<2)
        // and tell the slave "you are cascade identity 2".
        outb(PIC1_DATA, 4);
        io_wait();
        outb(PIC2_DATA, 2);
        io_wait();

        // ICW4: put both PICs in 8086 mode.
        outb(PIC1_DATA, MODE_8086);
        io_wait();
        outb(PIC2_DATA, MODE_8086);
        io_wait();

        // Masks (1 = masked/disabled). Unmask the timer (IRQ0, Milestone 9b) and
        // the keyboard (IRQ1, Milestone 5) on the master; mask the cascade and
        // everything on the slave for now. 0xff & ~0b11 = 0xFC.
        outb(PIC1_DATA, 0xff & !((1 << 1) | (1 << 0)));
        outb(PIC2_DATA, 0xff);
    }
}

/// Acknowledge an interrupt so the PIC will deliver the next one. Without this,
/// the PIC assumes the current IRQ is still being serviced and goes silent.
///
/// For an IRQ handled by the slave PIC (8..15) both PICs must be told; for a
/// master IRQ (0..7, including the keyboard) only the master.
pub fn send_eoi(irq: u8) {
    // SAFETY: writing the fixed EOI command to the PIC command port(s).
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, CMD_EOI);
        }
        outb(PIC1_COMMAND, CMD_EOI);
    }
}
