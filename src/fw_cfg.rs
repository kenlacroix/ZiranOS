//! Minimal QEMU `fw_cfg` reader — just enough to fetch one named entry.
//!
//! `fw_cfg` is QEMU's firmware-configuration channel: a 16-bit **selector** port
//! (0x510) picks an item, then successive byte reads from the **data** port (0x511)
//! stream that item's contents. This is how the **Tier-2 CTF host injects a
//! per-session secret into a guest it otherwise cannot see** — `qemu … -fw_cfg
//! name=opt/flag,string=FLAG{…}` on the host, read here as `opt/flag`. The secret
//! never lives in the guest image or the repo; only in that instance's RAM.
//!
//! On an ordinary boot with no such entry (e.g. the in-browser Tier-1 kernel),
//! [`read_flag`] returns `None` and the kernel stays in its strict-filesystem mode.
//!
//! Multi-byte fields in the file **directory** are big-endian (the fw_cfg
//! convention); an item's payload is raw bytes.

use crate::port::{inb, outw};
use alloc::vec::Vec;

const SELECTOR: u16 = 0x510; // write a 16-bit item selector here
const DATA: u16 = 0x511; //     then read the selected item's bytes here, in order
const FILE_DIR: u16 = 0x0019; // selector of the file directory (name -> selector)
const DIR_NAME_LEN: usize = 56; // width of the name field in a directory entry

// Sanity caps. A real fw_cfg directory has a few dozen entries and small items;
// these bound the work if the device ever reports garbage — an absent or odd
// fw_cfg returning all-ones would otherwise make `count`/`size` ~4 billion and
// hang the boot reading the data port forever (or OOM on the allocation). The
// caps keep this reader as total as the rest of the kernel's hostile-input
// parsing (cf. fs.rs), and they also protect the public browser boot, which now
// runs this at startup. A legitimate `opt/flag` is tiny, so neither cap is close.
const MAX_ENTRIES: usize = 4096;
const MAX_ITEM_BYTES: usize = 64 * 1024;

// SAFETY (applies to every port access below): a `fw_cfg` read only advances the
// selected item's read cursor and has no other side effect; selecting an item is
// idempotent and resets that cursor. The ports are the fixed QEMU `fw_cfg`
// registers. Every read is bounded by a count or size the hardware itself reports,
// so we never read unbounded or past an item.
fn select(sel: u16) {
    unsafe { outw(SELECTOR, sel) };
}
fn data_byte() -> u8 {
    unsafe { inb(DATA) }
}
fn data_bytes(n: usize) -> Vec<u8> {
    (0..n).map(|_| data_byte()).collect()
}
fn be_u32() -> u32 {
    u32::from_be_bytes([data_byte(), data_byte(), data_byte(), data_byte()])
}
fn be_u16() -> u16 {
    u16::from_be_bytes([data_byte(), data_byte()])
}

/// Fetch the bytes of a named `fw_cfg` file, or `None` if there is no such entry
/// (the normal case when the guest wasn't launched with one). Total and panic-free:
/// it reads only what the directory's own count and sizes describe.
pub fn read_file(name: &str) -> Option<Vec<u8>> {
    // The file directory: a big-endian u32 count, then that many 64-byte entries of
    // { size: be u32, select: be u16, reserved: u16, name: [u8; 56] (NUL-padded) }.
    select(FILE_DIR);
    // Cap the walk: a garbage count must not turn this into an unbounded read.
    let count = (be_u32() as usize).min(MAX_ENTRIES);
    for _ in 0..count {
        // Cap the item size too: it bounds the one allocation we make on a match.
        let size = (be_u32() as usize).min(MAX_ITEM_BYTES);
        let sel = be_u16();
        let _reserved = be_u16();
        let raw = data_bytes(DIR_NAME_LEN);
        let ename = match raw.iter().position(|&c| c == 0) {
            Some(z) => &raw[..z],
            None => &raw[..],
        };
        if ename == name.as_bytes() {
            select(sel); // re-select the found item; its cursor resets to 0
            return Some(data_bytes(size));
        }
    }
    None
}

/// The Tier-2 per-session flag the CTF host injected via `-fw_cfg opt/flag`, if any.
pub fn read_flag() -> Option<Vec<u8>> {
    read_file("opt/flag")
}
