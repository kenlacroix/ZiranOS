//! VGA text-mode output — Milestone 3, "Making it talk back."
//!
//! In text mode the BIOS/hardware exposes an 80x25 grid of characters at the
//! fixed physical address `0xb8000`. Each cell is two bytes: an ASCII code
//! point and a colour attribute (background nibble << 4 | foreground nibble).
//! Writing a byte pair there makes a glyph appear — no driver, no BIOS call,
//! no syscall. This module is the whole "graphics stack" for now.

use core::fmt::{self, Write};
use spin::Mutex;

const VGA_BUFFER: *mut u8 = 0xb8000 as *mut u8;
const BUFFER_WIDTH: usize = 80;
const BUFFER_HEIGHT: usize = 25;

/// The 16 colours the VGA text attribute byte can encode.
#[allow(dead_code)]
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

/// Pack a foreground and background colour into the single attribute byte the
/// hardware expects.
const fn attr(fg: Color, bg: Color) -> u8 {
    (bg as u8) << 4 | (fg as u8)
}

const DEFAULT_ATTR: u8 = attr(Color::LightGreen, Color::Black);

/// A cursor over the VGA text buffer. Tracks where the next character goes and
/// knows how to scroll when it runs off the bottom.
pub struct Writer {
    col: usize,
    row: usize,
    attr: u8,
}

impl Writer {
    /// Write a single cell (character + attribute) at `(row, col)`.
    ///
    /// SAFETY note: `0xb8000` is memory-mapped hardware, so every access must be
    /// `volatile` — the compiler must not assume it can cache, reorder, or elide
    /// these writes just because nothing in *our* address space reads them back.
    fn put_cell(&self, row: usize, col: usize, byte: u8, attribute: u8) {
        let offset = (row * BUFFER_WIDTH + col) * 2;
        unsafe {
            core::ptr::write_volatile(VGA_BUFFER.add(offset), byte);
            core::ptr::write_volatile(VGA_BUFFER.add(offset + 1), attribute);
        }
    }

    fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.col >= BUFFER_WIDTH {
                    self.new_line();
                }
                self.put_cell(self.row, self.col, byte, self.attr);
                self.col += 1;
            }
        }
    }

    fn new_line(&mut self) {
        self.col = 0;
        if self.row + 1 < BUFFER_HEIGHT {
            self.row += 1;
        } else {
            self.scroll_up();
        }
    }

    /// Move every row up by one and blank the bottom row. Straightforward
    /// byte-copy of the mapped buffer; keeps the newest output visible.
    fn scroll_up(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let src = (row * BUFFER_WIDTH + col) * 2;
                let dst = ((row - 1) * BUFFER_WIDTH + col) * 2;
                unsafe {
                    let ch = core::ptr::read_volatile(VGA_BUFFER.add(src));
                    let at = core::ptr::read_volatile(VGA_BUFFER.add(src + 1));
                    core::ptr::write_volatile(VGA_BUFFER.add(dst), ch);
                    core::ptr::write_volatile(VGA_BUFFER.add(dst + 1), at);
                }
            }
        }
        self.blank_row(BUFFER_HEIGHT - 1);
        self.row = BUFFER_HEIGHT - 1;
    }

    fn blank_row(&self, row: usize) {
        for col in 0..BUFFER_WIDTH {
            self.put_cell(row, col, b' ', self.attr);
        }
    }

    fn clear(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.blank_row(row);
        }
        self.col = 0;
        self.row = 0;
    }
}

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // The VGA font only covers 0x00..=0xff; anything outside printable
            // ASCII (e.g. multi-byte UTF-8 from our banner) becomes a filled box
            // so we never emit an undefined glyph.
            let out = match byte {
                0x20..=0x7e | b'\n' => byte,
                _ => 0xfe,
            };
            self.write_byte(out);
        }
        Ok(())
    }
}

/// The single global writer, guarded by a spinlock because we have no real
/// scheduler or mutex yet. `Mutex` here is a busy-wait, which is fine for a
/// single core with interrupts off.
static WRITER: Mutex<Writer> = Mutex::new(Writer {
    col: 0,
    row: 0,
    attr: DEFAULT_ATTR,
});

/// Clear the screen and reset the cursor to the top-left.
pub fn clear_screen() {
    WRITER.lock().clear();
}

/// Backs the `print!`/`println!` macros. Not called directly.
#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // `write_fmt` cannot actually fail for our `Writer`, but the trait returns a
    // Result, so we discard it explicitly.
    let _ = WRITER.lock().write_fmt(args);
}

/// Print to the VGA text buffer (no trailing newline).
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::vga_buffer::_print(format_args!($($arg)*)));
}

/// Print a line to the VGA text buffer.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
