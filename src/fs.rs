//! ZranFS — a minimal read-only filesystem over a RAM disk — Milestone 11.
//!
//! A "file" here is a lie the header tells. The whole disk is one flat `&[u8]`,
//! and a small directory table says "pretend the bytes `[offset..offset+length]`
//! are a file named N". That is *all* a filesystem fundamentally is: a run of
//! bytes plus a map from names to extents.
//!
//! The milestone is the round-trip. [`boot_image`] is the *writer* (a tiny
//! in-kernel `mkfs`): it packs a fixed set of files into a `Vec<u8>` on the heap,
//! writing every field as little-endian bytes. [`Fs::mount`] is the *reader*: it
//! re-derives the whole directory from those raw bytes, **trusting nothing but the
//! array** — no struct is shared across the boundary. `mount` is the entire trust
//! boundary: it checks the lie is self-consistent (magic, version, and that no
//! entry's extent escapes the disk), so that after it succeeds, [`Fs::list`] and
//! [`Fs::read`] are total and panic-free.
//!
//! There is no `unsafe` in this module: it is all safe slice indexing over a heap
//! `Vec`. The interesting hazard is therefore *logical* out-of-bounds (a bad
//! offset in the header), not memory-unsafe out-of-bounds — `mount` turns the
//! former into a clean `Err`, and safe Rust would turn an unchecked slice into a
//! loud panic, never silent corruption. See `docs/concepts/filesystem.md`.
//!
//! We deliberately chose this custom format over FAT16: a valid FAT16 image needs
//! `mkfs.fat` (a host tool + an opaque prebuilt blob — the "build magic" the
//! project forbids), whereas ZranFS builds its disk in readable Rust. FAT16 read
//! is a deferred future exercise; see PLAN.md §7 and the M11 eng-plan.

use alloc::string::String;
use alloc::vec::Vec;

/// The signature at offset 0: ASCII `ZRFS`. The first thing `mount` checks.
const MAGIC: [u8; 4] = *b"ZRFS";
/// The only on-disk format version this reader understands. v2 added the `kind`
/// byte (subdirectories, Milestone 12); a v1 image now mounts as
/// `UnsupportedVersion` (we keep only the current reader).
const VERSION: u16 = 2;
/// Superblock size, in bytes. The directory table starts right after it.
const SUPERBLOCK_LEN: usize = 16;
/// Directory-entry size, in bytes.
const DIRENT_LEN: usize = 32;
/// Filename field width within a directory entry (NUL-padded).
const NAME_LEN: usize = 20;

// Field offsets within a directory entry.
const ENT_OFFSET: usize = 0x14; // u32: byte offset of the node's region
const ENT_LENGTH: usize = 0x18; // u32: region length in bytes
const ENT_KIND: usize = 0x1C; // u32: 0 = file, 1 = directory (was v1 `reserved`)

/// A directory entry names a plain file: its region is opaque bytes.
const KIND_FILE: u32 = 0;
/// A directory entry names a subdirectory: its region is itself an array of
/// `length / DIRENT_LEN` directory entries — the same lie, recursively.
const KIND_DIR: u32 = 1;

/// Why an image failed to mount. Each variant is one validation step — and one
/// deliberately-corrupt input the self-test feeds to prove the reader parses
/// rather than coincidentally succeeds (the seed of the M16 fuzzing track).
#[derive(Debug, PartialEq, Eq)]
pub enum FsError {
    /// The first four bytes are not `ZRFS`.
    BadMagic,
    /// The version field is not one this reader supports.
    UnsupportedVersion,
    /// The image is too short for the superblock, or for the directory table the
    /// header claims (including the integer-overflow case).
    Truncated,
    /// The header's `total_size` field disagrees with the actual image length —
    /// the disk's own story about its size doesn't match the disk.
    SizeMismatch,
    /// A directory entry's `offset + length` runs past the end of the image (or
    /// overflows). The headline safety check: without it `read` would slice out
    /// of bounds.
    EntryOutOfBounds,
    /// A subdirectory's region length is not a whole number of 32-byte entries.
    BadDirLength,
    /// A subdirectory's region does not start at/after the end of its parent —
    /// the forward-ordering rule that makes validation *terminate*. Any cycle,
    /// back-edge to an ancestor, or pointer into the superblock trips this.
    DirNotForward,
    /// A directory entry's `kind` field is neither file (0) nor directory (1).
    BadKind,
    /// A path component names nothing in its directory (resolution-time).
    NotFound,
    /// A path descended through something that is a file, not a directory, or
    /// `list_dir`/`cd` was given a file path (resolution-time).
    NotADirectory,
    /// `read_path`/`cat` was given a directory path (resolution-time).
    IsADirectory,
}

/// What a path resolves to: a file's byte region, or a directory's entry region.
#[derive(Debug, PartialEq, Eq)]
pub enum Node {
    File { offset: usize, length: usize },
    Dir { offset: usize, length: usize },
}

/// One directory entry, as `ls` renders it.
#[derive(Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub offset: usize,
    pub length: usize,
    pub is_dir: bool,
}

/// A validated, read-only view over a RAM-disk image. It borrows the image; every
/// bound was checked in [`Fs::mount`], so its methods never panic and never fail.
pub struct Fs<'a> {
    image: &'a [u8],
    file_count: usize,
}

impl<'a> Fs<'a> {
    /// Parse and *fully validate* an image. This is the whole trust boundary:
    /// every bound is checked here, in order, each mapping to one [`FsError`], so
    /// [`list`](Fs::list)/[`read`](Fs::read) can be total.
    pub fn mount(image: &'a [u8]) -> Result<Fs<'a>, FsError> {
        if image.len() < SUPERBLOCK_LEN {
            return Err(FsError::Truncated);
        }
        if image[..4] != MAGIC {
            return Err(FsError::BadMagic);
        }
        if read_u16_le(image, 0x04) != VERSION {
            return Err(FsError::UnsupportedVersion);
        }
        // The header's total_size must match the actual image length — the field's
        // self-consistency cross-check, and one more thing a corrupt image trips.
        if read_u32_le(image, 0x08) as usize != image.len() {
            return Err(FsError::SizeMismatch);
        }
        let file_count = read_u16_le(image, 0x06) as usize;

        // The directory table must fit. `checked_mul`/`checked_add` guard the
        // classic overflow-to-a-small-number parse bug (a huge file_count).
        let dir_bytes = file_count.checked_mul(DIRENT_LEN).ok_or(FsError::Truncated)?;
        let dir_end = SUPERBLOCK_LEN.checked_add(dir_bytes).ok_or(FsError::Truncated)?;
        if dir_end > image.len() {
            return Err(FsError::Truncated);
        }

        // Validate the whole directory tree, rooted at the root table. Every
        // entry's extent is checked in-bounds (as in v1), and every subdirectory
        // is recursed into — but only if its region starts at/after its parent's
        // end (the forward-ordering rule), which makes the recursion provably
        // terminate and rejects cycles. `forward_min = SUPERBLOCK_LEN` means a
        // top-level subdirectory can't alias the superblock or the root table.
        //
        // Still-deliberate non-checks (M16 seeds): a FILE extent is checked
        // in-bounds but not confined to the data region and may alias another
        // file's or the metadata's bytes; only directory *tables* are kept from
        // overlapping (a free consequence of forward-ordering).
        validate_dir(image, SUPERBLOCK_LEN, file_count * DIRENT_LEN, SUPERBLOCK_LEN)?;

        Ok(Fs { image, file_count })
    }

    /// Resolve a **canonical absolute path** (`"/"`, `"/docs"`, `"/docs/x.txt"`)
    /// to the node it names. The path must already be canonical — the shell does
    /// all `.`/`..` handling as pure string math (see `shell::canonicalize`), so
    /// this only ever *descends*: no parent pointers, no cycles to chase.
    pub fn resolve(&self, abs: &str) -> Result<Node, FsError> {
        let mut node = Node::Dir { offset: SUPERBLOCK_LEN, length: self.file_count * DIRENT_LEN };
        for comp in abs.split('/') {
            if comp.is_empty() {
                continue; // leading '/', or a stray '//'
            }
            let (doff, dlen) = match node {
                Node::Dir { offset, length } => (offset, length),
                Node::File { .. } => return Err(FsError::NotADirectory),
            };
            node = self.find_child(doff, dlen, comp).ok_or(FsError::NotFound)?;
        }
        Ok(node)
    }

    /// Find a named entry within the directory region `[doff, doff+dlen)`, as a
    /// `Node`. Post-mount, every `kind` is a validated 0 or 1, so a non-`DIR` kind
    /// is a file.
    fn find_child(&self, doff: usize, dlen: usize, name: &str) -> Option<Node> {
        for i in 0..(dlen / DIRENT_LEN) {
            let base = doff + i * DIRENT_LEN;
            if entry_name(&self.image[base..base + NAME_LEN]) == name {
                let offset = read_u32_le(self.image, base + ENT_OFFSET) as usize;
                let length = read_u32_le(self.image, base + ENT_LENGTH) as usize;
                return Some(match read_u32_le(self.image, base + ENT_KIND) {
                    KIND_DIR => Node::Dir { offset, length },
                    _ => Node::File { offset, length },
                });
            }
        }
        None
    }

    /// List the directory at `abs`: name/size/kind for each entry. What `ls`
    /// renders. `NotADirectory` if the path is a file.
    pub fn list_dir(&self, abs: &str) -> Result<Vec<DirEntry>, FsError> {
        let (off, len) = match self.resolve(abs)? {
            Node::Dir { offset, length } => (offset, length),
            Node::File { .. } => return Err(FsError::NotADirectory),
        };
        Ok((0..(len / DIRENT_LEN))
            .map(|i| {
                let base = off + i * DIRENT_LEN;
                DirEntry {
                    name: entry_name(&self.image[base..base + NAME_LEN]),
                    offset: read_u32_le(self.image, base + ENT_OFFSET) as usize,
                    length: read_u32_le(self.image, base + ENT_LENGTH) as usize,
                    is_dir: read_u32_le(self.image, base + ENT_KIND) == KIND_DIR,
                }
            })
            .collect())
    }

    /// Read the file at `abs` — a zero-copy slice into the image. `IsADirectory`
    /// if the path names a directory, `NotFound` if nothing.
    pub fn read_path(&self, abs: &str) -> Result<&'a [u8], FsError> {
        match self.resolve(abs)? {
            // Copy the `&'a [u8]` out of `self` so the slice carries the image's
            // lifetime, not this `&self` borrow.
            Node::File { offset, length } => {
                let image = self.image;
                Ok(&image[offset..offset + length])
            }
            Node::Dir { .. } => Err(FsError::IsADirectory),
        }
    }
}

/// Recursively validate one directory region `[off, off + len)`: every entry's
/// extent must lie in the image, and every subdirectory must begin at/after
/// `forward_min` — the end of its parent's region — before we recurse into it.
///
/// **Termination proof.** Along any root-to-descendant descent, the `forward_min`
/// passed down equals the parent region's end, and a non-empty parent has
/// `end > off`, so each child's `off >= end > parent.off`: the descent's start
/// offsets are strictly increasing. A strictly increasing sequence of
/// non-negative integers all `< image.len()` has length `<= image.len()`, so the
/// recursion depth is bounded and the walk always terminates — no depth cap, no
/// visited-set, no possibility of looping on a malformed (cyclic) image.
fn validate_dir(image: &[u8], off: usize, len: usize, forward_min: usize) -> Result<(), FsError> {
    if off < forward_min {
        return Err(FsError::DirNotForward); // cycle / back-edge / into the superblock
    }
    if len % DIRENT_LEN != 0 {
        return Err(FsError::BadDirLength); // not a whole array of entries
    }
    let end = off.checked_add(len).ok_or(FsError::EntryOutOfBounds)?;
    if end > image.len() {
        return Err(FsError::EntryOutOfBounds);
    }
    for i in 0..(len / DIRENT_LEN) {
        let base = off + i * DIRENT_LEN;
        let c_off = read_u32_le(image, base + ENT_OFFSET) as usize;
        let c_len = read_u32_le(image, base + ENT_LENGTH) as usize;
        let c_end = c_off.checked_add(c_len).ok_or(FsError::EntryOutOfBounds)?;
        if c_end > image.len() {
            return Err(FsError::EntryOutOfBounds);
        }
        match read_u32_le(image, base + ENT_KIND) {
            KIND_FILE => {} // a file's region only has to be in-bounds
            KIND_DIR => validate_dir(image, c_off, c_len, end)?, // child must be >= this region's end
            _ => return Err(FsError::BadKind),
        }
    }
    Ok(())
}

/// Read a little-endian `u16` at `off`. The caller (only `mount`, over a
/// length-checked image) guarantees `off + 2 <= buf.len()`.
fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

/// Read a little-endian `u32` at `off`. `mount` validates the directory table
/// fits before reading any entry field, so this never indexes out of bounds.
fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Decode a fixed-width, NUL-padded name field into a `String`. If the field has
/// no NUL (the name fills all `NAME_LEN` bytes) the whole field is the name.
/// `from_utf8_lossy` keeps a non-ASCII name from panicking.
fn entry_name(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// Write one 32-byte directory entry (name, offset, length, kind) to `img`.
fn write_entry(img: &mut Vec<u8>, name: &str, offset: u32, length: u32, kind: u32) {
    let mut namebuf = [0u8; NAME_LEN];
    let nb = name.as_bytes();
    let take = nb.len().min(NAME_LEN);
    namebuf[..take].copy_from_slice(&nb[..take]);
    img.extend_from_slice(&namebuf); // +0x00 name
    img.extend_from_slice(&offset.to_le_bytes()); // +0x14 offset
    img.extend_from_slice(&length.to_le_bytes()); // +0x18 length
    img.extend_from_slice(&kind.to_le_bytes()); // +0x1C kind
}

/// Build the boot RAM disk: a valid ZranFS **v2 tree**. This is the *writer* — it
/// shares no structs with [`Fs`], just lays down little-endian bytes. The tree is:
///
/// ```text
/// /                       (root: motd.txt, readme, ziran.txt, docs/)
/// └── docs/               (filesystem.txt, shell.txt — the OS carrying a bit of
///                          its own explanation)
/// [hidden]  b"FLAG{...}"   bytes in the image with NO directory entry — the M16
///                          exfil target: present, yet unreachable by ls/cat.
/// ```
///
/// Physical layout satisfies the forward-ordering rule automatically: superblock,
/// then the root table, then the `docs/` child table (which starts exactly at the
/// root table's end), then all file data, then the secret.
pub fn boot_image() -> Vec<u8> {
    let root_files: &[(&str, &[u8])] = &[
        ("motd.txt", b"Hello, Ziran!"),
        ("readme", b"Ziran OS -- a from-scratch x86_64 kernel.\n"),
        ("ziran.txt", "\u{81ea}\u{7136} (ziran): that which arises without external forcing.\n".as_bytes()),
    ];
    let docs_files: &[(&str, &[u8])] = &[
        ("filesystem.txt", b"A file is a lie a header tells about bytes.\n"),
        ("shell.txt", b"cd, pwd, ls, cat -- navigation over ZranFS v2.\n"),
    ];
    // The planted secret: bytes with no directory entry pointing at them (§8).
    const SECRET: &[u8] = b"FLAG{ziran-boundary-leak}";

    let root_count = root_files.len() + 1; // + the docs/ directory entry
    let docs_count = docs_files.len();

    let root_table_off = SUPERBLOCK_LEN;
    let docs_table_off = root_table_off + root_count * DIRENT_LEN;
    let data_off = docs_table_off + docs_count * DIRENT_LEN;

    let data_total: usize = root_files.iter().map(|(_, b)| b.len()).sum::<usize>()
        + docs_files.iter().map(|(_, b)| b.len()).sum::<usize>();
    let total = data_off + data_total + SECRET.len();

    let mut img = Vec::with_capacity(total);

    // Superblock.
    img.extend_from_slice(&MAGIC);
    img.extend_from_slice(&VERSION.to_le_bytes());
    img.extend_from_slice(&(root_count as u16).to_le_bytes());
    img.extend_from_slice(&(total as u32).to_le_bytes());
    img.extend_from_slice(&0u32.to_le_bytes()); // reserved

    // Root table. Data offsets accumulate as we go: root files' data comes first
    // (right after both tables), then docs files' data.
    let mut cursor = data_off;
    for (name, bytes) in root_files {
        write_entry(&mut img, name, cursor as u32, bytes.len() as u32, KIND_FILE);
        cursor += bytes.len();
    }
    write_entry(&mut img, "docs", docs_table_off as u32, (docs_count * DIRENT_LEN) as u32, KIND_DIR);

    // docs/ child table (its files' data follows the root files' data).
    for (name, bytes) in docs_files {
        write_entry(&mut img, name, cursor as u32, bytes.len() as u32, KIND_FILE);
        cursor += bytes.len();
    }

    // Data region: root files, then docs files (matching the offsets above).
    for (_, bytes) in root_files {
        img.extend_from_slice(bytes);
    }
    for (_, bytes) in docs_files {
        img.extend_from_slice(bytes);
    }

    // The hidden secret, last — inside the image, named by nothing.
    img.extend_from_slice(SECRET);

    debug_assert_eq!(img.len(), total, "fs: boot_image size mismatch");
    img
}

/// Prove the filesystem over serial, deterministically, with no keyboard: mount
/// the boot image, navigate the directory tree and check a file's *exact* bytes,
/// confirm the planted secret is unreachable, and — the honest gate — reject
/// eight deliberately-corrupt images, each with its specific error. A reader that
/// returns the right bytes on a good image, refuses eight malformations (a
/// directory *cycle* among them, without looping), and cannot reach the hidden
/// bytes is provably parsing, not coincidentally working.
pub fn self_test() {
    let image = boot_image();

    // Happy path: mount the tree, read a root file's exact bytes.
    let fs = Fs::mount(&image).expect("fs: boot image failed to mount");
    let root = fs.list_dir("/").expect("fs: root not listable");
    assert!(
        root.iter().any(|e| e.name == "motd.txt" && e.length == 13 && !e.is_dir),
        "fs: motd.txt missing or wrong size"
    );
    assert!(root.iter().any(|e| e.name == "docs" && e.is_dir), "fs: docs/ missing");
    let motd = fs.read_path("/motd.txt").expect("fs: /motd.txt unreadable");
    assert_eq!(motd, &b"Hello, Ziran!"[..], "fs: motd.txt bytes wrong");

    // Navigate into the subdirectory and read a file byte-for-byte.
    assert!(matches!(fs.resolve("/docs"), Ok(Node::Dir { .. })), "fs: /docs not a dir");
    let docs = fs.list_dir("/docs").expect("fs: /docs not listable");
    assert_eq!(docs.len(), 2, "fs: /docs should have 2 files");
    let fsdoc = fs.read_path("/docs/filesystem.txt").expect("fs: subdir file unreadable");
    assert_eq!(
        fsdoc,
        &b"A file is a lie a header tells about bytes.\n"[..],
        "fs: subdir file bytes wrong"
    );

    // Resolution-time errors are specific, not panics.
    assert_eq!(fs.read_path("/docs").err(), Some(FsError::IsADirectory));
    assert_eq!(fs.resolve("/nope").err(), Some(FsError::NotFound));
    assert_eq!(fs.list_dir("/motd.txt").err(), Some(FsError::NotADirectory));

    // The boundary holds: the planted secret has no directory entry, so NO path
    // reaches it — it is present in the image yet unreachable by ls/cat. (M16's
    // job is to find the crafted input that would leak it.)
    assert_eq!(fs.read_path("/FLAG").err(), Some(FsError::NotFound));
    assert!(fs.read_path("/docs/FLAG").is_err(), "fs: secret must be unreachable");

    // Corrupt-input rejections — each asserts the *specific* FsError.
    let mut bad_magic = image.clone();
    bad_magic[0] = b'X';
    assert_eq!(Fs::mount(&bad_magic).err(), Some(FsError::BadMagic));

    let mut bad_version = image.clone();
    bad_version[4] = 99;
    assert_eq!(Fs::mount(&bad_version).err(), Some(FsError::UnsupportedVersion));

    assert_eq!(Fs::mount(&image[..8]).err(), Some(FsError::Truncated));

    // Header's total_size no longer matches the real length.
    let mut bad_size = image.clone();
    bad_size[8] = bad_size[8].wrapping_add(1);
    assert_eq!(Fs::mount(&bad_size).err(), Some(FsError::SizeMismatch));

    // Point entry 0's offset past the end of the disk.
    let mut oob = image.clone();
    let off_field = SUPERBLOCK_LEN + ENT_OFFSET;
    oob[off_field..off_field + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert_eq!(Fs::mount(&oob).err(), Some(FsError::EntryOutOfBounds));

    // --- Milestone 12: subdirectory-tree validation rejections ---
    let kind0 = SUPERBLOCK_LEN + ENT_KIND; // entry 0's kind field
    let off0 = SUPERBLOCK_LEN + ENT_OFFSET;
    let len0 = SUPERBLOCK_LEN + ENT_LENGTH;
    let file_count = read_u16_le(&image, 0x06) as usize;
    let root_end = (SUPERBLOCK_LEN + file_count * DIRENT_LEN) as u32;

    // A subdirectory whose region points back at the root table is a cycle: the
    // forward-ordering rule rejects it (rather than looping forever) — the
    // milestone's headline safety property.
    let mut cycle = image.clone();
    cycle[kind0..kind0 + 4].copy_from_slice(&KIND_DIR.to_le_bytes());
    cycle[off0..off0 + 4].copy_from_slice(&(SUPERBLOCK_LEN as u32).to_le_bytes());
    cycle[len0..len0 + 4].copy_from_slice(&(DIRENT_LEN as u32).to_le_bytes());
    assert_eq!(Fs::mount(&cycle).err(), Some(FsError::DirNotForward));

    // A subdirectory whose length isn't a whole number of 32-byte entries.
    let mut bad_len = image.clone();
    bad_len[kind0..kind0 + 4].copy_from_slice(&KIND_DIR.to_le_bytes());
    bad_len[off0..off0 + 4].copy_from_slice(&root_end.to_le_bytes());
    bad_len[len0..len0 + 4].copy_from_slice(&33u32.to_le_bytes());
    assert_eq!(Fs::mount(&bad_len).err(), Some(FsError::BadDirLength));

    // An entry whose kind is neither file (0) nor directory (1).
    let mut bad_kind = image.clone();
    bad_kind[kind0..kind0 + 4].copy_from_slice(&99u32.to_le_bytes());
    assert_eq!(Fs::mount(&bad_kind).err(), Some(FsError::BadKind));

    crate::serial_println!(
        "[ok] fs: mounted a v2 tree ({} root entries incl. docs/), read /docs/filesystem.txt \
         byte-for-byte, secret unreachable, 8 corrupt images rejected (incl. a cycle, no hang)",
        root.len()
    );
    crate::serial_println!("M11: filesystem online");
}
