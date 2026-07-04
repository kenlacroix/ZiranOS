//! The interactive shell — Milestone 10.
//!
//! This is the first thing the kernel runs that *feels* like a program: a
//! read-eval-print loop you type at. The lesson of the milestone is in how input
//! reaches it. The keyboard IRQ does not run the shell; it only drops each byte
//! into a lock-free ring (`keyboard::push`). The shell is an ordinary **task** on
//! the Milestone 9 scheduler that *consumes* that ring — echoing characters,
//! editing a line, and running commands. "Interactive" turns out to be nothing
//! more than a task polling a queue an interrupt fills.
//!
//! When its ring is empty the shell executes `hlt` rather than spinning or
//! `yield_now`-ing: with only the shell and the idle task runnable, `yield_now`
//! would no-op and a spin would peg the core, whereas `hlt` sleeps the CPU until
//! the next interrupt — a keystroke wakes it at once, the 100 Hz timer at worst
//! ~10 ms later. See `docs/concepts/input-and-shell.md`.

use crate::keyboard;
use crate::{interrupts, print, println, serial_println};

/// Maximum length of one input line. Fixed and heapless: a longer line simply
/// stops accepting printable characters (no wrap, no reallocation).
const LINE_MAX: usize = 128;

/// The prompt, reprinted after every line.
const PROMPT: &str = "ziran> ";

/// The shell's editable input line — a fixed byte buffer, all ASCII.
struct Line {
    buf: [u8; LINE_MAX],
    len: usize,
}

/// What a byte did to the line — separates the *pure* editing logic (testable
/// without any I/O) from the echoing the REPL does in response.
#[derive(PartialEq, Eq, Debug)]
enum Edit {
    /// A printable byte was appended; echo it.
    Echo(u8),
    /// A character was erased; echo a backspace.
    Erase,
    /// Enter was pressed; the line is complete and ready to dispatch.
    Submit,
    /// Nothing happened (buffer full, backspace on an empty line, or a stray
    /// control byte).
    Ignored,
}

impl Line {
    const fn new() -> Line {
        Line { buf: [0; LINE_MAX], len: 0 }
    }

    /// Fold one input byte into the line and report what happened. Pure — no I/O —
    /// so the REPL can echo and the self-test can assert.
    fn edit(&mut self, byte: u8) -> Edit {
        match byte {
            b'\n' => Edit::Submit,
            // Backspace: only if there is something to erase — never walk back over
            // the prompt.
            0x08 => {
                if self.len > 0 {
                    self.len -= 1;
                    Edit::Erase
                } else {
                    Edit::Ignored
                }
            }
            // Printable ASCII: append if there is room.
            0x20..=0x7e => {
                if self.len < LINE_MAX {
                    self.buf[self.len] = byte;
                    self.len += 1;
                    Edit::Echo(byte)
                } else {
                    Edit::Ignored
                }
            }
            _ => Edit::Ignored,
        }
    }

    /// The current line as a string slice (always valid UTF-8: only printable
    /// ASCII is ever stored).
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    fn clear(&mut self) {
        self.len = 0;
    }
}

/// The shell task entry point. Spawned onto the scheduler by `kernel_main`; runs
/// forever (never returns). `extern "C"` to match `task::Task::new`'s signature.
pub extern "C" fn shell_main() {
    // A freshly spawned task is reached via `switch_context`'s `ret`, not `iretq`,
    // so it starts with interrupts disabled. Enable them — otherwise the `hlt`
    // below would wedge forever and no keystroke IRQ could ever wake us.
    interrupts::enable();

    serial_println!("M10: shell task online");
    println!();
    println!("Ziran OS shell. (commands arrive in the next step -- for now it echoes.)");
    print!("{PROMPT}");

    let mut line = Line::new();
    loop {
        match keyboard::pop() {
            Some(byte) => match line.edit(byte) {
                Edit::Echo(b) => print!("{}", b as char),
                Edit::Erase => print!("\u{8}"), // the VGA writer moves back and blanks the cell
                Edit::Submit => {
                    println!();
                    // (command dispatch on `line.as_str()` lands here in step 2)
                    line.clear();
                    print!("{PROMPT}");
                }
                Edit::Ignored => {}
            },
            // Ring empty: sleep until the next interrupt re-runs this loop. Safe
            // because the shell holds no lock here and runs with interrupts on.
            // SAFETY: `hlt` only pauses this CPU until the next interrupt.
            None => unsafe {
                core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
            },
        }
    }
}

/// Prove the line editor over serial, without a keyboard: feed a synthetic byte
/// stream and assert the resulting buffer and the `Edit` outcomes. This is the
/// deterministic half of the milestone's verification — interactive typing is
/// checked by hand under `make run`, but the editing *logic* is pinned here.
pub fn self_test() {
    let mut line = Line::new();

    // Type "hi", backspace once, type "at" -> "hat".
    assert_eq!(line.edit(b'h'), Edit::Echo(b'h'));
    assert_eq!(line.edit(b'i'), Edit::Echo(b'i'));
    assert_eq!(line.edit(0x08), Edit::Erase);
    assert_eq!(line.edit(b'a'), Edit::Echo(b'a'));
    assert_eq!(line.edit(b't'), Edit::Echo(b't'));
    assert_eq!(line.as_str(), "hat", "shell: line editing produced the wrong buffer");

    // Backspace never underflows past an empty line.
    let mut empty = Line::new();
    assert_eq!(empty.edit(0x08), Edit::Ignored);
    assert_eq!(empty.len, 0, "shell: backspace underflowed an empty line");

    // Enter submits and the line resets cleanly.
    assert_eq!(line.edit(b'\n'), Edit::Submit);
    line.clear();
    assert_eq!(line.len, 0);

    serial_println!("[ok] shell: line editing (echo, backspace, submit) verified");
}
