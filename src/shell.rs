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
    Load,
    Net,
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
        // `load` takes its (large) payload by streaming the console itself, not
        // from this line — see `cmd_load`. Any tail here is ignored.
        "load" => Command::Load,
        "net" => Command::Net,
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
            shln!("  load          mount a base64 image pasted over the console");
            shln!("  net           send 'hello' over the loopback interface");
        }
        Command::Echo(rest) => shln!("{rest}"),
        Command::Clear => crate::vga_buffer::clear_screen(),
        Command::Mem => cmd_mem(),
        Command::Ps => cmd_ps(),
        Command::Pwd => cmd_pwd(cwd),
        Command::Cd(p) => cmd_cd(cwd, p),
        Command::Ls(p) => cmd_ls(cwd, p),
        Command::Cat(p) => cmd_cat(cwd, p),
        Command::Load => cmd_load(),
        Command::Net => crate::net::demo(),
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
    let image = active_image();
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
    let image = active_image();
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
    let image = active_image();
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

// ---------------------------------------------------------------------------
// `load` — mount a user-supplied image (Milestone 16, Tier-1 CTF delivery).
//
// Lets a visitor paste a *crafted* filesystem image and then drive `ls`/`cat`
// over it. The exploit that makes this a challenge lives one layer down: the
// player controls the whole image including the superblock, so they can forge
// `data_end = total_size` and alias a file's extent over bytes with no directory
// entry — the documented weakness in `Fs::mount` ("validating against a value the
// caller controls is not validation"). This command is only the delivery vehicle;
// it mounts through the real, strict reader, exactly like the boot disk.
// ---------------------------------------------------------------------------

/// The image the FS commands operate on. `None` until a successful `load`, after
/// which `ls`/`cat`/`cd` see the uploaded image instead of the built-in boot disk.
/// Only the shell task ever touches this, and no interrupt handler reaches it, so
/// the spinlock never contends — it is here to make the shared-state borrow plainly
/// sound, not to arbitrate a race.
static LOADED: spin::Mutex<Option<Vec<u8>>> = spin::Mutex::new(None);

/// Largest image `load` accepts (decoded). A hostile paste is stopped the instant
/// it crosses this, *before* any mount, so an enormous declared `total_size` cannot
/// be used to exhaust the heap.
const MAX_IMAGE: usize = 64 * 1024;

/// The bytes the FS commands should mount right now: the uploaded image if one has
/// been `load`ed, else the built-in boot disk. Returns an owned copy so the caller
/// mounts a local slice — the same "mount fresh each command" discipline `ls`/`cat`
/// already use (the image is small and capped, so the clone is cheap).
fn active_image() -> Vec<u8> {
    match &*LOADED.lock() {
        Some(img) => img.clone(),
        None => crate::fs::boot_image(),
    }
}

/// Why a base64 payload was rejected — distinct so `load` can name the fault, the
/// way the FS reports a specific `FsError`.
#[derive(PartialEq, Eq, Debug)]
enum B64Error {
    /// A byte outside the standard alphabet (after whitespace was stripped).
    InvalidChar,
    /// The alphabet-only length is not a multiple of 4, or `=` padding is malformed.
    BadLength,
}

/// Standard base64 symbol -> its 6-bit value; `None` for `=` and any non-symbol.
fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode standard base64 (with `=` padding) to raw bytes. Input must already be
/// alphabet+`=` only — `load` strips whitespace as it reads. Total and panic-free:
/// every malformed length, misplaced pad, or stray byte returns a `B64Error`, never
/// a panic — the same discipline the FS `mount` boundary holds, because this too
/// parses fully hostile input.
fn b64_decode(input: &[u8]) -> Result<Vec<u8>, B64Error> {
    if input.len() % 4 != 0 {
        return Err(B64Error::BadLength);
    }
    let chunks = input.len() / 4;
    let mut out = Vec::with_capacity(chunks * 3);
    for (i, chunk) in input.chunks_exact(4).enumerate() {
        let last = i + 1 == chunks;
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for (j, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                // `=` is legal only as trailing padding in the final chunk (the
                // last one or two positions).
                if !last || j < 2 {
                    return Err(B64Error::BadLength);
                }
                pad += 1;
            } else {
                // A real symbol after a pad byte means the pad wasn't trailing.
                if pad != 0 {
                    return Err(B64Error::BadLength);
                }
                vals[j] = b64_val(c).ok_or(B64Error::InvalidChar)?;
            }
        }
        let triple = (vals[0] as u32) << 18
            | (vals[1] as u32) << 12
            | (vals[2] as u32) << 6
            | (vals[3] as u32);
        out.push((triple >> 16) as u8); //   pad 0,1,2 -> always the first byte
        if pad < 2 {
            out.push((triple >> 8) as u8); // pad 0,1   -> a second byte
        }
        if pad < 1 {
            out.push(triple as u8); //        pad 0     -> a third byte
        }
    }
    Ok(out)
}

/// Encode bytes as standard base64. Used only by the self-test to round-trip a real
/// image back through [`b64_decode`].
fn b64_encode(input: &[u8]) -> Vec<u8> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(A[(n >> 18 & 63) as usize]);
        out.push(A[(n >> 12 & 63) as usize]);
        out.push(if chunk.len() > 1 { A[(n >> 6 & 63) as usize] } else { b'=' });
        out.push(if chunk.len() > 2 { A[(n & 63) as usize] } else { b'=' });
    }
    out
}

/// `load` — replace the active disk with a base64 image pasted over the console.
/// The line editor caps a line at `LINE_MAX` (128) bytes, far smaller than an
/// image, so `load` takes over the read loop itself: it streams bytes straight from
/// the same keyboard/serial sources, keeping base64 symbols, skipping whitespace,
/// and stopping at a `.` sentinel (a period never appears in base64). The payload
/// is size-capped *as it arrives*, decoded, then validated by the real `Fs::mount`
/// before it is trusted — a bad image is rejected with its `FsError` and the
/// previous image is left untouched.
fn cmd_load() {
    shln!("load: paste a base64 filesystem image, then a line with a single '.'");

    // Base64 expands 3 bytes to 4, so cap the encoded stream a little above the
    // decoded ceiling; a payload past it is drained (not stored) until the sentinel.
    let b64_cap = MAX_IMAGE / 3 * 4 + 4;
    let mut payload: Vec<u8> = Vec::new();
    let mut overflowed = false;
    loop {
        match keyboard::pop().or_else(crate::serial::read_byte) {
            Some(b'.') => break, // sentinel — end of image
            Some(b) if b.is_ascii_whitespace() => {} // layout only; ignore
            Some(b) => {
                if payload.len() >= b64_cap {
                    overflowed = true; // keep reading to the '.', but stop storing
                } else {
                    payload.push(b);
                }
            }
            // Busy-poll rather than `hlt`: a fast paste over the UART's ~16-byte FIFO
            // would overflow if we only woke on the 100 Hz tick. `load` is a short
            // burst, so draining continuously is worth the spin.
            None => core::hint::spin_loop(),
        }
    }

    if overflowed {
        shln!("load: image too large (max {MAX_IMAGE} bytes)");
        return;
    }
    let bytes = match b64_decode(&payload) {
        Ok(b) => b,
        Err(e) => {
            shln!("load: invalid base64 ({e:?})");
            return;
        }
    };
    if bytes.is_empty() {
        shln!("load: empty image (nothing pasted)");
        return;
    }
    if bytes.len() > MAX_IMAGE {
        shln!("load: image too large (max {MAX_IMAGE} bytes)");
        return;
    }
    // Validate through the *real* reader before trusting it — never store an image
    // that would make the FS commands fail to mount.
    if let Err(e) = crate::fs::Fs::mount(&bytes) {
        shln!("load: not a valid image: {e:?}");
        return;
    }
    let n = bytes.len();
    *LOADED.lock() = Some(bytes);
    shln!("load: mounted {n}-byte image (ls / cat / cd now read it)");
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
    assert_eq!(parse("net"), Command::Net);

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
    assert_eq!(parse("load"), Command::Load);
    assert_eq!(parse("load ignored tail"), Command::Load); // tail is streamed, not parsed
    assert_eq!(parse("bogus xyz"), Command::Unknown("bogus"));

    // base64 (the `load` payload codec): known vectors incl. every padding case,
    // and clean rejection of malformed input — never a panic, mirroring the FS
    // mount boundary since this too decodes hostile bytes.
    assert_eq!(b64_decode(b"").unwrap(), b"");
    assert_eq!(b64_decode(b"TWFu").unwrap(), b"Man"); // no padding
    assert_eq!(b64_decode(b"TWE=").unwrap(), b"Ma"); //  one pad
    assert_eq!(b64_decode(b"TQ==").unwrap(), b"M"); //   two pads
    assert_eq!(b64_decode(b"aGVsbG8=").unwrap(), b"hello");
    assert_eq!(b64_decode(b"ABC"), Err(B64Error::BadLength)); //   not a multiple of 4
    assert_eq!(b64_decode(b"=AAA"), Err(B64Error::BadLength)); //  pad not trailing
    assert_eq!(b64_decode(b"TW=u"), Err(B64Error::BadLength)); //  symbol after a pad
    assert_eq!(b64_decode(b"A!==").err(), Some(B64Error::InvalidChar)); // stray byte
    // Round-trip a *real* image through encode -> decode and confirm it still mounts
    // — this is exactly the path `load` drives.
    let img = crate::fs::boot_image();
    let round = b64_decode(&b64_encode(&img)).unwrap();
    assert_eq!(round, img, "shell: base64 round-trip corrupted a real image");
    assert!(
        crate::fs::Fs::mount(&round).is_ok(),
        "shell: round-tripped image won't mount"
    );
    // With nothing loaded, the FS commands see the boot disk.
    assert_eq!(active_image(), img, "shell: active_image should default to the boot disk");

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
