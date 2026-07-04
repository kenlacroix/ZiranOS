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
//! would just bounce to idle and back every tick and a spin would peg the core,
//! whereas `hlt` sleeps the CPU until the next interrupt. If the shell is the
//! current task its keystroke IRQ wakes it at once; if a tick has parked it and
//! made idle current, the byte waits in the ring until the next tick reschedules
//! the shell — bounded by the 100 Hz period, never lost. See
//! `docs/concepts/input-and-shell.md`.

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

/// A parsed command line: the verb, plus the argument tail where relevant. Kept
/// as a pure parse result (borrowing the input line) so command *recognition* is
/// testable without running any I/O.
#[derive(PartialEq, Eq, Debug)]
enum Command<'a> {
    Empty,
    Help,
    Echo(&'a str),
    Clear,
    Mem,
    Ps,
    Unknown(&'a str),
}

/// Split a line into a command and its argument tail. The whole line is trimmed
/// first (so leading and trailing whitespace are dropped), then the first
/// whitespace-run separates the verb from the rest. Internal spacing in the tail
/// is preserved, so `echo a  b` keeps its double space; trailing spaces do not
/// survive (`echo` cannot emit them).
fn parse(line: &str) -> Command<'_> {
    let line = line.trim();
    if line.is_empty() {
        return Command::Empty;
    }
    let (verb, rest) = match line.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim_start()),
        None => (line, ""),
    };
    match verb {
        "help" => Command::Help,
        "echo" => Command::Echo(rest),
        "clear" => Command::Clear,
        "mem" => Command::Mem,
        "ps" => Command::Ps,
        other => Command::Unknown(other),
    }
}

/// Run a completed line. Output goes to the VGA console (the human's screen).
fn dispatch(line: &str) {
    match parse(line) {
        Command::Empty => {}
        Command::Help => {
            println!("commands:");
            println!("  help          show this list");
            println!("  echo <text>   print <text>");
            println!("  clear         clear the screen");
            println!("  mem           show memory usage");
            println!("  ps            list tasks");
        }
        Command::Echo(rest) => println!("{rest}"),
        Command::Clear => crate::vga_buffer::clear_screen(),
        Command::Mem => cmd_mem(),
        Command::Ps => cmd_ps(),
        Command::Unknown(verb) => println!("unknown command: {verb} (try help)"),
    }
}

/// `mem` — live physical-frame and heap usage, plus uptime from the timer.
fn cmd_mem() {
    let free_frames = crate::frame_allocator::free_frame_count();
    let (heap_used, heap_free) = crate::heap::stats();
    let ticks = crate::pit::ticks();
    println!("memory:");
    println!("  frames: {free_frames} free ({} MiB)", free_frames / 256);
    println!(
        "  heap:   {heap_used} used / {heap_free} free / {} total bytes",
        crate::heap::HEAP_SIZE
    );
    println!("  uptime: {} s ({ticks} ticks @ 100 Hz)", ticks / 100);
}

/// `ps` — the scheduler's task list: id, state, and which one is current.
fn cmd_ps() {
    let tasks = crate::task::snapshot();
    println!("tasks: {}", tasks.len());
    for (id, state, current) in tasks {
        let marker = if current { "  (current)" } else { "" };
        println!("  [{id}] {}{marker}", state.name());
    }
}

/// The shell task entry point. Spawned onto the scheduler by `kernel_main`; runs
/// forever (never returns). `extern "C"` to match `task::Task::new`'s signature.
pub extern "C" fn shell_main() {
    // A freshly spawned task is reached via `switch_context`'s `ret`, not `iretq`,
    // so it starts with interrupts disabled. Enable them — otherwise the `hlt`
    // below would wedge forever and no keystroke IRQ could ever wake us.
    interrupts::enable();

    serial_println!("M10: shell online");
    println!();
    println!("Ziran OS shell -- type `help` for commands.");
    print!("{PROMPT}");

    let mut line = Line::new();
    loop {
        match keyboard::pop() {
            Some(byte) => match line.edit(byte) {
                Edit::Echo(b) => print!("{}", b as char),
                Edit::Erase => print!("\u{8}"), // the VGA writer moves back and blanks the cell
                Edit::Submit => {
                    println!();
                    dispatch(line.as_str());
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

    // Command parsing: each verb classifies correctly, the echo tail is preserved
    // verbatim, and an unknown verb is reported (not silently swallowed).
    assert_eq!(parse(""), Command::Empty);
    assert_eq!(parse("   "), Command::Empty);
    assert_eq!(parse("help"), Command::Help);
    assert_eq!(parse("clear"), Command::Clear);
    assert_eq!(parse("mem"), Command::Mem);
    assert_eq!(parse("ps"), Command::Ps);
    assert_eq!(parse("echo hello  world"), Command::Echo("hello  world"));
    assert_eq!(parse("echo"), Command::Echo(""));
    assert_eq!(parse("bogus xyz"), Command::Unknown("bogus"));

    // The input ring (the milestone's crux) round-trips FIFO. Guard with
    // interrupts off so the real keyboard IRQ — the only other producer — cannot
    // push concurrently and break the single-producer contract during the test.
    // Interrupts have been live since `sti`, so a key pressed earlier in boot may
    // already sit in the ring; drain it first rather than assert emptiness (which
    // a stray keystroke would fail), then test the bytes this test itself pushes.
    let flags = interrupts::save_and_disable();
    while keyboard::pop().is_some() {}
    keyboard::push(b'x');
    keyboard::push(b'y');
    assert_eq!(keyboard::pop(), Some(b'x'), "shell: input ring not FIFO");
    assert_eq!(keyboard::pop(), Some(b'y'));
    assert_eq!(keyboard::pop(), None, "shell: input ring should be drained");
    interrupts::restore(flags);

    // The data accessors behind `ps`/`mem` return sane, live values (asserted here
    // on serial; the formatted commands print to VGA and are checked by hand).
    let tasks = crate::task::snapshot();
    assert!(!tasks.is_empty(), "shell: task snapshot empty");
    assert!(
        tasks.iter().any(|&(id, _, _)| id == 0),
        "shell: idle task (id 0) missing from ps"
    );
    let (used, free) = crate::heap::stats();
    assert!(
        free > 0 && free <= crate::heap::HEAP_SIZE as usize,
        "shell: heap free out of range"
    );
    assert!(used < crate::heap::HEAP_SIZE as usize, "shell: heap fully used?");
    assert!(
        crate::frame_allocator::free_frame_count() > 0,
        "shell: no free frames"
    );

    serial_println!(
        "[ok] shell: editing, parsing, input ring, and ps/mem accessors verified \
         (tasks={}, heap_free={})",
        tasks.len(),
        free
    );
}
