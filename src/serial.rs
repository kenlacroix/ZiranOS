//! Serial port (16550 UART on COM1) output.
//!
//! The VGA buffer is great for a human looking at a QEMU window, but useless to
//! an automated boot test: nothing can read those glyphs back. The serial port
//! is the classic answer. QEMU's `-serial stdio` pipes COM1 straight to the
//! terminal, so a headless CI run (or `make run-headless`) can capture the
//! kernel's output and assert on it. This is what makes "does it still boot?"
//! a checkable question.
//!
//! We talk to the UART directly through the legacy port-I/O space (`in`/`out`
//! instructions), programming it exactly as firmware would: 38400 baud, 8N1.

use crate::port::{inb, outb};
use core::fmt::{self, Write};
use spin::Mutex;

const COM1: u16 = 0x3f8;

struct SerialPort {
    base: u16,
    initialised: bool,
}

impl SerialPort {
    const fn new(base: u16) -> Self {
        SerialPort { base, initialised: false }
    }

    /// Standard 16550 init dance: disable interrupts, set the divisor for
    /// 38400 baud, choose 8 bits / no parity / 1 stop bit, enable and clear the
    /// FIFOs. Done once, lazily, on first use.
    fn init(&mut self) {
        unsafe {
            outb(self.base + 1, 0x00); // disable all UART interrupts
            outb(self.base + 3, 0x80); // enable DLAB (set baud rate divisor)
            outb(self.base + 0, 0x03); // divisor low byte  -> 38400 baud
            outb(self.base + 1, 0x00); // divisor high byte
            outb(self.base + 3, 0x03); // 8 bits, no parity, one stop bit
            outb(self.base + 2, 0xc7); // enable FIFO, clear, 14-byte threshold
            outb(self.base + 4, 0x0b); // IRQs enabled, RTS/DSR set
        }
        self.initialised = true;
    }

    fn is_transmit_empty(&self) -> bool {
        // Bit 5 of the line-status register: transmitter holding register empty.
        unsafe { inb(self.base + 5) & 0x20 != 0 }
    }

    fn send(&mut self, byte: u8) {
        if !self.initialised {
            self.init();
        }
        // Translate a bare LF into CRLF so terminals render lines sanely.
        if byte == b'\n' {
            self.send_raw(b'\r');
        }
        self.send_raw(byte);
    }

    fn send_raw(&self, byte: u8) {
        while !self.is_transmit_empty() {
            core::hint::spin_loop();
        }
        unsafe { outb(self.base, byte) }
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.send(byte);
        }
        Ok(())
    }
}

static SERIAL1: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1));

/// Backs the `serial_print!`/`serial_println!` macros. Not called directly.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = SERIAL1.lock().write_fmt(args);
}

/// Print to COM1 (no trailing newline).
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::serial::_print(format_args!($($arg)*)));
}

/// Print a line to COM1.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}
