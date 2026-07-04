//! The interactive shell — Milestone 10 (with `ls`/`cat` over the RAM disk added
//! in Milestone 11).
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
use crate::{interrupts, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// Print to **both** the VGA console and the serial line. All shell output goes
/// through these, so the shell is usable whether you watch the QEMU window (legacy
/// BIOS) or drive it over a serial terminal — the only console available under
/// UEFI/OVMF, where the legacy VGA text buffer isn't displayed. `serial::read_byte`
/// is the matching input path (see `shell_main`).
macro_rules! shp {
    ($($a:tt)*) => {{
        crate::print!($($a)*);
        crate::serial_print!($($a)*);
    }};
}
macro_rules! shln {
    () => {{ crate::println!(); crate::serial_println!(); }};
    ($($a:tt)*) => {{
        crate::println!($($a)*);
        crate::serial_println!($($a)*);
    }};
}

/// Maximum length of one input line. Fixed and heapless: a longer line simply
/// stops accepting printable characters (no wrap, no reallocation).
const LINE_MAX: usize = 128;

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
    Pwd,
    Cd(&'a str),
    Ls(&'a str),
    Cat(&'a str),
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
        "pwd" => Command::Pwd,
        // A path has no spaces in this FS, so take just the first token of the tail.
        "cd" => Command::Cd(rest.split_whitespace().next().unwrap_or("")),
        "ls" => Command::Ls(rest.split_whitespace().next().unwrap_or("")),
        "cat" => Command::Cat(rest.split_whitespace().next().unwrap_or("")),
        other => Command::Unknown(other),
    }
}

/// Run a completed line against the current working directory. Output goes to the
/// VGA console (the human's screen). Only `cd` mutates `cwd`.
fn dispatch(cwd: &mut String, line: &str) {
    match parse(line) {
        Command::Empty => {}
        Command::Help => {
            shln!("commands:");
            shln!("  help          show this list");
            shln!("  echo <text>   print <text>");
            shln!("  clear         clear the screen");
            shln!("  mem           show memory usage");
            shln!("  ps            list tasks");
            shln!("  pwd           print the working directory");
            shln!("  cd [path]     change directory (no arg -> /)");
            shln!("  ls [path]     list a directory (default: cwd)");
            shln!("  cat <path>    print a file");
        }
        Command::Echo(rest) => shln!("{rest}"),
        Command::Clear => crate::vga_buffer::clear_screen(),
        Command::Mem => cmd_mem(),
        Command::Ps => cmd_ps(),
        Command::Pwd => cmd_pwd(cwd),
        Command::Cd(p) => cmd_cd(cwd, p),
        Command::Ls(p) => cmd_ls(cwd, p),
        Command::Cat(p) => cmd_cat(cwd, p),
        Command::Unknown(verb) => shln!("unknown command: {verb} (try help)"),
    }
}

/// `mem` — live physical-frame and heap usage, plus uptime from the timer.
fn cmd_mem() {
    let free_frames = crate::frame_allocator::free_frame_count();
    let (heap_used, heap_free) = crate::heap::stats();
    let ticks = crate::pit::ticks();
    shln!("memory:");
    shln!("  frames: {free_frames} free ({} MiB)", free_frames / 256);
    shln!(
        "  heap:   {heap_used} used / {heap_free} free / {} total bytes",
        crate::heap::HEAP_SIZE
    );
    shln!("  uptime: {} s ({ticks} ticks @ 100 Hz)", ticks / 100);
}

/// `ps` — the scheduler's task list: id, state, and which one is current.
fn cmd_ps() {
    let tasks = crate::task::snapshot();
    shln!("tasks: {}", tasks.len());
    for (id, state, current) in tasks {
        let marker = if current { "  (current)" } else { "" };
        shln!("  [{id}] {}{marker}", state.name());
    }
}

/// Join `arg` onto `cwd` and normalize `.`/`..`/`//` into a canonical absolute
/// path. `cwd` must already be canonical (leading `/`). **Pure — no I/O** — so
/// `..` is a `Vec::pop`, never a disk parent-pointer walk, and the whole thing is
/// unit-testable. The result is always canonical: leading `/`, no `.`/`..`, and no
/// trailing slash except the root, which is `"/"`.
fn canonicalize(cwd: &str, arg: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    // An absolute arg ignores cwd; a relative one starts from it.
    let base = if arg.starts_with('/') { "" } else { cwd };
    for part in base.split('/').chain(arg.split('/')) {
        match part {
            "" | "." => {}                // leading/double/trailing slash, or "."
            ".." => { stack.pop(); }      // pop; on empty stack (root) it's a no-op
            name => stack.push(name),
        }
    }
    if stack.is_empty() {
        String::from("/")
    } else {
        let mut out = String::new();
        for name in &stack {
            out.push('/');
            out.push_str(name);
        }
        out
    }
}

/// Print the prompt for the current directory, e.g. `ziran:/docs> `.
fn print_prompt(cwd: &str) {
    shp!("ziran:{cwd}> ");
}

/// `pwd` — print the current working directory.
fn cmd_pwd(cwd: &str) {
    shln!("{cwd}");
}

/// `cd [path]` — change directory. No arg -> root. A bad `cd` leaves cwd unchanged.
fn cmd_cd(cwd: &mut String, arg: &str) {
    if arg.is_empty() {
        cwd.clear();
        cwd.push('/');
        return;
    }
    let target = canonicalize(cwd, arg);
    let image = crate::fs::boot_image();
    let fs = match crate::fs::Fs::mount(&image) {
        Ok(fs) => fs,
        Err(e) => {
            shln!("cd: cannot mount filesystem: {e:?}");
            return;
        }
    };
    match fs.resolve(&target) {
        Ok(crate::fs::Node::Dir { .. }) => *cwd = target,
        Ok(crate::fs::Node::File { .. }) => shln!("cd: not a directory: {target}"),
        Err(_) => shln!("cd: no such directory: {target}"),
    }
}

/// `ls [path]` — list a directory (the cwd if no path). Subdirectories are tagged
/// `<dir>`; files show their size. Mounts fresh each time (the image is ~350 bytes
/// and stateless, so there is nothing to cache).
fn cmd_ls(cwd: &str, arg: &str) {
    let target = if arg.is_empty() { cwd.into() } else { canonicalize(cwd, arg) };
    let image = crate::fs::boot_image();
    let fs = match crate::fs::Fs::mount(&image) {
        Ok(fs) => fs,
        Err(e) => {
            shln!("ls: cannot mount filesystem: {e:?}");
            return;
        }
    };
    match fs.list_dir(&target) {
        Ok(entries) if entries.is_empty() => shln!("(empty)"),
        Ok(entries) => {
            for e in entries {
                if e.is_dir {
                    shln!("  {:20} <dir>", e.name);
                } else {
                    shln!("  {:20} {} bytes", e.name, e.length);
                }
            }
        }
        Err(_) => shln!("ls: no such directory: {target}"),
    }
}

/// `cat <path>` — print a file's bytes, resolved relative to cwd. Non-UTF-8 bytes
/// are shown lossily rather than panicking (on-disk data is arbitrary bytes).
fn cmd_cat(cwd: &str, arg: &str) {
    if arg.is_empty() {
        shln!("usage: cat <path>");
        return;
    }
    let target = canonicalize(cwd, arg);
    let image = crate::fs::boot_image();
    let fs = match crate::fs::Fs::mount(&image) {
        Ok(fs) => fs,
        Err(e) => {
            shln!("cat: cannot mount filesystem: {e:?}");
            return;
        }
    };
    match fs.read_path(&target) {
        Ok(bytes) => shp!("{}", String::from_utf8_lossy(bytes)),
        Err(crate::fs::FsError::IsADirectory) => shln!("cat: is a directory: {target}"),
        Err(_) => shln!("no such file: {target}"),
    }
}

/// The shell task entry point. Spawned onto the scheduler by `kernel_main`; runs
/// forever (never returns). `extern "C"` to match `task::Task::new`'s signature.
pub extern "C" fn shell_main() {
    // A freshly spawned task is reached via `switch_context`'s `ret`, not `iretq`,
    // so it starts with interrupts disabled. Enable them — otherwise the `hlt`
    // below would wedge forever and no keystroke IRQ could ever wake us.
    interrupts::enable();

    serial_println!("M12: file manager online");
    shln!();
    shln!("Ziran OS shell -- type `help` for commands.");

    let mut cwd = String::from("/");
    print_prompt(&cwd);

    let mut line = Line::new();
    loop {
        // Take input from the PS/2 keyboard ring OR the serial line (a terminal),
        // whichever has a byte. Serial has no IRQ here (we left UART interrupts
        // off), so it is polled; the 100 Hz timer wakes the `hlt` below, so serial
        // input lands within one tick — imperceptible to a typist.
        match keyboard::pop().or_else(crate::serial::read_byte) {
            Some(raw) => {
                // Normalize terminal conventions to what `Line::edit` expects: a
                // serial Enter is CR (0x0d), and terminals send DEL (0x7f) for
                // Backspace.
                let byte = match raw {
                    b'\r' => b'\n',
                    0x7f => 0x08,
                    b => b,
                };
                match line.edit(byte) {
                    Edit::Echo(b) => shp!("{}", b as char),
                    // Erase visibly on a serial terminal too: back, space, back.
                    // On VGA this nets the same (the writer blanks on each \u{8}).
                    Edit::Erase => shp!("\u{8} \u{8}"),
                    Edit::Submit => {
                        shln!();
                        dispatch(&mut cwd, line.as_str());
                        line.clear();
                        print_prompt(&cwd);
                    }
                    Edit::Ignored => {}
                }
            }
            // Nothing to read: sleep until the next interrupt (timer tick or a
            // keystroke IRQ) re-runs this loop. Safe — the shell holds no lock here
            // and runs with interrupts on.
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
    assert_eq!(parse("pwd"), Command::Pwd);
    assert_eq!(parse("cd"), Command::Cd(""));
    assert_eq!(parse("cd /docs"), Command::Cd("/docs"));
    assert_eq!(parse("cd .."), Command::Cd(".."));
    assert_eq!(parse("ls"), Command::Ls(""));
    assert_eq!(parse("ls /docs"), Command::Ls("/docs"));
    assert_eq!(parse("cat docs/x"), Command::Cat("docs/x"));
    assert_eq!(parse("cat"), Command::Cat("")); // no filename -> empty, handled by cmd_cat
    assert_eq!(parse("cat a b"), Command::Cat("a")); // only the first token (a path has no spaces)

    // Path normalization (the M12 load-bearing pure logic).
    assert_eq!(canonicalize("/docs", ".."), "/");
    assert_eq!(canonicalize("/", ".."), "/"); // pop past root is a no-op
    assert_eq!(canonicalize("/", "docs"), "/docs");
    assert_eq!(canonicalize("/a/b", "../c"), "/a/c");
    assert_eq!(canonicalize("/a/b", "/x/y"), "/x/y"); // absolute ignores cwd
    assert_eq!(canonicalize("/docs", "."), "/docs");
    assert_eq!(canonicalize("/docs", "x/"), "/docs/x"); // trailing slash dropped
    assert_eq!(canonicalize("/a", "../../b"), "/b"); // extra ".." no-ops at root
    assert_eq!(canonicalize("/docs", ""), "/docs"); // empty arg = stay
    assert_eq!(canonicalize("/", "//x//"), "/x"); // empty components dropped
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
        "[ok] shell: editing, parsing, path normalization, input ring, and ps/mem \
         accessors verified (tasks={}, heap_free={})",
        tasks.len(),
        free
    );
}
