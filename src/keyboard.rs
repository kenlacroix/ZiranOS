//! PS/2 keyboard driver — Milestone 5.
//!
//! When you press a key, the keyboard controller makes a byte available at I/O
//! port 0x60 (a "scancode") and raises IRQ1. Our interrupt handler calls
//! `handle_interrupt`, which reads that byte and turns it into a character.
//!
//! Scancodes come in "sets"; the PC default is **set 1**. A key *press* sends a
//! "make code" (e.g. 0x1E for 'A'); a key *release* sends the same code with the
//! high bit set — its "break code" (0x9E). We care about presses, plus the
//! make/break of Shift so we can pick upper- vs lower-case.
//!
//! Reading port 0x60 is also what lets the controller send the *next* byte, so
//! we must always read it, even for keys we don't map.

use crate::port::inb;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

const DATA_PORT: u16 = 0x60;

// Set-1 make/break codes for the two Shift keys.
const LSHIFT_MAKE: u8 = 0x2a;
const RSHIFT_MAKE: u8 = 0x36;
const LSHIFT_BREAK: u8 = 0xaa;
const RSHIFT_BREAK: u8 = 0xb6;

/// Whether a Shift key is currently held. An `AtomicBool` (not a lock) because
/// this is touched only from the keyboard interrupt, and we never want a driver
/// to risk blocking inside an interrupt handler.
static SHIFT: AtomicBool = AtomicBool::new(false);

/// Read one scancode and return the character it produced, if any.
///
/// Returns `None` for key releases, modifier keys, and any key we don't map —
/// but always reads the port first so the controller keeps flowing.
pub fn handle_interrupt() -> Option<char> {
    // SAFETY: reading the fixed PS/2 data port; the read also acknowledges the
    // byte to the keyboard controller.
    let code = unsafe { inb(DATA_PORT) };

    match code {
        LSHIFT_MAKE | RSHIFT_MAKE => {
            SHIFT.store(true, Ordering::Relaxed);
            None
        }
        LSHIFT_BREAK | RSHIFT_BREAK => {
            SHIFT.store(false, Ordering::Relaxed);
            None
        }
        // Any other break code (high bit set) is a key release we ignore.
        _ if code & 0x80 != 0 => None,
        _ => translate(code, SHIFT.load(Ordering::Relaxed)),
    }
}

/// Translate a set-1 make code to a character, honoring Shift. Covers the main
/// alphanumeric block and common punctuation of a US QWERTY layout; unmapped
/// keys (function keys, arrows, keypad, etc.) return `None`.
fn translate(code: u8, shift: bool) -> Option<char> {
    // Keys whose output doesn't depend on Shift.
    match code {
        0x0e => return Some('\u{8}'), // Backspace
        0x1c => return Some('\n'),    // Enter
        0x39 => return Some(' '),     // Space
        _ => {}
    }

    // (unshifted, shifted) for the rest. Letters map lower/upper; the number row
    // and punctuation map to their shifted symbols.
    let (lo, hi) = match code {
        0x02 => ('1', '!'),
        0x03 => ('2', '@'),
        0x04 => ('3', '#'),
        0x05 => ('4', '$'),
        0x06 => ('5', '%'),
        0x07 => ('6', '^'),
        0x08 => ('7', '&'),
        0x09 => ('8', '*'),
        0x0a => ('9', '('),
        0x0b => ('0', ')'),
        0x0c => ('-', '_'),
        0x0d => ('=', '+'),
        0x10 => ('q', 'Q'),
        0x11 => ('w', 'W'),
        0x12 => ('e', 'E'),
        0x13 => ('r', 'R'),
        0x14 => ('t', 'T'),
        0x15 => ('y', 'Y'),
        0x16 => ('u', 'U'),
        0x17 => ('i', 'I'),
        0x18 => ('o', 'O'),
        0x19 => ('p', 'P'),
        0x1a => ('[', '{'),
        0x1b => (']', '}'),
        0x1e => ('a', 'A'),
        0x1f => ('s', 'S'),
        0x20 => ('d', 'D'),
        0x21 => ('f', 'F'),
        0x22 => ('g', 'G'),
        0x23 => ('h', 'H'),
        0x24 => ('j', 'J'),
        0x25 => ('k', 'K'),
        0x26 => ('l', 'L'),
        0x27 => (';', ':'),
        0x28 => ('\'', '"'),
        0x29 => ('`', '~'),
        0x2b => ('\\', '|'),
        0x2c => ('z', 'Z'),
        0x2d => ('x', 'X'),
        0x2e => ('c', 'C'),
        0x2f => ('v', 'V'),
        0x30 => ('b', 'B'),
        0x31 => ('n', 'N'),
        0x32 => ('m', 'M'),
        0x33 => (',', '<'),
        0x34 => ('.', '>'),
        0x35 => ('/', '?'),
        _ => return None,
    };

    Some(if shift { hi } else { lo })
}

// --- Input ring (Milestone 10) ----------------------------------------------
//
// The keyboard IRQ is a *producer* and the shell task is a *consumer*, so
// keystrokes flow through a lock-free single-producer/single-consumer ring of
// bytes rather than being echoed inside the handler. This is the house rule made
// concrete: state touched by an interrupt uses atomics, never a lock — a handler
// must never risk blocking. With exactly one producer and one consumer, each
// index has a single writer, so no lock is needed at all.
//
// `push` (the IRQ) publishes a byte by storing it and then advancing `TAIL` with
// a Release; `pop` (the shell) reads `TAIL` with an Acquire before touching the
// byte, so it never observes a slot before the byte lands in it. On one core this
// really forbids *compiler* reordering — there is no second core to race — but the
// Acquire/Release pair states the contract correctly regardless.

/// Ring capacity (power of two). 256 bytes is far more backlog than a human
/// typist can build against a shell that drains every timer tick.
const RING_CAP: usize = 256;

/// The ring storage. One `AtomicU8` per slot; the index ordering fences them, so
/// the bytes themselves use `Relaxed`.
static RING: [AtomicU8; RING_CAP] = [const { AtomicU8::new(0) }; RING_CAP];
/// Consumer cursor — only the shell task advances it.
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// Producer cursor — only the keyboard IRQ advances it.
static TAIL: AtomicUsize = AtomicUsize::new(0);

/// Enqueue one input byte. Called **only** from the keyboard IRQ, so it is
/// lock-free and never blocks. A full ring drops the newest byte — bounded and
/// honest; a keystroke lost under a 256-byte backlog is a non-event.
pub fn push(byte: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % RING_CAP;
    if next == HEAD.load(Ordering::Acquire) {
        return; // full: drop the newest keystroke rather than overwrite unread data
    }
    RING[tail].store(byte, Ordering::Relaxed);
    TAIL.store(next, Ordering::Release); // publish only after the byte is written
}

/// Dequeue one input byte, or `None` if the ring is empty. Called **only** from
/// the shell task.
pub fn pop() -> Option<u8> {
    let head = HEAD.load(Ordering::Relaxed);
    if head == TAIL.load(Ordering::Acquire) {
        return None; // empty
    }
    let byte = RING[head].load(Ordering::Relaxed);
    HEAD.store((head + 1) % RING_CAP, Ordering::Release);
    Some(byte)
}
