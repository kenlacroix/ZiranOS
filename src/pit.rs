//! The Programmable Interval Timer (8253/8254) — Milestone 9b.
//!
//! This is the kernel's first sense of *time*. The PIT is a tiny chip that
//! counts down from a value we choose at a fixed 1.193182 MHz, and — in the mode
//! we use — raises IRQ0 each time it reaches zero, then reloads and does it
//! again. Program the reload value and you choose the interrupt *rate*; that
//! steady heartbeat is what drives preemptive scheduling (see `src/task.rs`).
//!
//! IRQ0 is remapped to vector 0x20 by the PIC (Milestone 5's `src/pic.rs`), so
//! the tick arrives through the exact same IDT → `interrupt_dispatch` → EOI path
//! the keyboard already uses. We deliberately use the PIT, not the local APIC
//! timer: it rides machinery that already exists (~15 lines), where the APIC
//! timer would be a milestone of MMIO and calibration on its own.
//!
//! See `docs/concepts/timer-and-scheduling.md`.

use crate::port::outb;
use core::sync::atomic::{AtomicU64, Ordering};

/// Channel-0 data port: the counter wired to IRQ0.
const PIT_CH0_DATA: u16 = 0x40;
/// The mode/command register (write-only).
const PIT_COMMAND: u16 = 0x43;
/// The PIT's fixed input frequency, in Hz (~1.193 MHz — a third of the old NTSC
/// colourburst crystal). Every channel counts down at this rate.
const PIT_FREQ_HZ: u32 = 1_193_182;

/// Ticks since the timer came online. An `AtomicU64` rather than a lock, for the
/// same reason the keyboard's shift state is atomic: it is touched from an
/// interrupt handler, which must never risk blocking on a lock.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Program channel 0 to raise IRQ0 at `target_hz`. Call once, after `pic::init`
/// (so the vector is remapped) and before enabling interrupts.
///
/// The divisor is `PIT_FREQ_HZ / target_hz`; the actual rate is
/// `PIT_FREQ_HZ / divisor`, exact to well under a percent at the rates we use.
pub fn init(target_hz: u32) {
    debug_assert!(target_hz != 0, "pit::init: target_hz must be non-zero");

    // The divisor is a 16-bit reload value, and the chip reads a written 0 as
    // 65536 (the slowest rate). Clamp so a future caller outside the sane band
    // (≤ 18 Hz would overflow 16 bits; 0 Hz would divide by zero) fails sane
    // rather than silently programming a garbage rate. Only `init(100)` is used
    // today, where `divisor_full` is 11931 (→ ~100.006 Hz).
    let divisor_full = (PIT_FREQ_HZ / target_hz.max(1)).clamp(1, 0x1_0000);
    let divisor = divisor_full as u16; // 0x1_0000 wraps to 0, which the PIT reads as 65536

    // SAFETY: this is the architecturally-fixed PIT programming sequence. The
    // command byte 0x34 = channel 0, lo/hi-byte access, mode 2 (rate generator —
    // one clean pulse per period, the honest "fire once per tick" model), binary
    // counting. Mode 2's access order requires the low byte written before the
    // high byte; writing them reversed would set a wildly wrong period.
    unsafe {
        outb(PIT_COMMAND, 0x34);
        outb(PIT_CH0_DATA, (divisor & 0xff) as u8);
        outb(PIT_CH0_DATA, (divisor >> 8) as u8);
    }

    crate::serial_println!(
        "[ok] timer: PIT channel 0 at ~{} Hz (divisor {})",
        PIT_FREQ_HZ / divisor_full,
        divisor_full
    );
}

/// Advance the tick counter. Called from the IRQ0 arm of `interrupt_dispatch`,
/// with interrupts already disabled (interrupt gate), so a relaxed atomic add is
/// all the synchronisation needed.
pub fn handle_interrupt() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Ticks observed since boot — the kernel's coarse clock.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}
