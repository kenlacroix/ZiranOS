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
/// byte (subdirectories, Milestone 12); v3 gives meaning to the reserved word as
/// `data_end` (the data-region ceiling that confines file extents — Milestone 16).
/// An older image now mounts as `UnsupportedVersion` (we keep only the current
/// reader). Each milestone grows the format by exactly one field.
const VERSION: u16 = 3;
/// Superblock size, in bytes. The directory table starts right after it.
const SUPERBLOCK_LEN: usize = 16;
/// Superblock field: `u32` at 0x0C, the exclusive end of the addressable data
/// region. Was v2's reserved word; v3 confines every file extent to `<= data_end`
/// so bytes past it (the planted secret) are unreachable through any entry.
const SB_DATA_END: usize = 0x0C;
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
    /// A **file** extent ends past `data_end` — inside the image, but outside the
    /// addressable data region, so it aliases the reserved bytes where the hidden
    /// secret lives. The Milestone 16 confinement check: `EntryOutOfBounds` keeps a
    /// read in the image; *this* keeps it inside the file data. Only the strict
    /// [`Fs::mount`] raises it; the loose [`Fs::mount_loose`] (the M11-era check)
    /// does not — which is exactly how the flag leaks.
    ExtentEscapesData,
    /// The superblock's `data_end` field is itself out of range (`> total_size` or
    /// `< SUPERBLOCK_LEN`). A malformed header, caught before it's trusted as the
    /// extent ceiling. (Note: a *valid-but-forged* `data_end` is not caught — see
    /// `Fs::mount`'s honest caveat.)
    BadDataEnd,
    /// A subdirectory's region length is not a whole number of 32-byte entries.
    BadDirLength,
    /// A subdirectory's region does not start at/after the end of its parent —
    /// the forward-ordering rule that makes validation *terminate*. Any cycle,
    /// back-edge to an ancestor, or pointer into the superblock trips this.
    DirNotForward,
    /// A directory entry's `kind` field is neither file (0) nor directory (1).
    BadKind,
    /// The directory tree nests deeper than `MAX_DEPTH` — a crafted deep chain
    /// that would otherwise overflow the (guard-page-less) kernel stack.
    TooDeep,
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
    /// Parse and *fully validate* an image (the strict reader — the whole trust
    /// boundary). Every bound is checked here, in order, each mapping to one
    /// [`FsError`], so [`list`](Fs::list)/[`read`](Fs::read) can be total. This
    /// confines every entry's extent — file data *and* directory tables — to
    /// `[.., data_end)`, so bytes past the declared data region (the planted secret)
    /// are unreachable through any entry.
    ///
    /// **Honest caveat (Milestone 16).** `data_end` lives *in the image*, so an
    /// attacker who can rewrite the whole **superblock** (not just a directory
    /// entry) could forge `data_end = total_size` and reopen the leak. `mount`
    /// confines the *entry* attack surface — the classic "corrupt a directory
    /// entry" case, and the documented M16 gap — and range-checks `data_end`, but
    /// it cannot make an in-image field trustworthy against a fully-forged header.
    /// Fully closing it needs structural coverage accounting or an out-of-band
    /// authority (out of scope). This is M15's deepest lesson one layer down:
    /// validating against a value the caller controls is not validation.
    pub fn mount(image: &'a [u8]) -> Result<Fs<'a>, FsError> {
        Self::mount_inner(image, true)
    }

    /// The **deliberately loose** reader: identical to [`mount`](Fs::mount) except
    /// it does *not* confine entry extents to `data_end` — it bounds them only by
    /// the image length, exactly as the Milestone 11 extent check did. (It still
    /// runs every other v3 check, including `data_end`'s range validation, so it is
    /// not a full M11 reader — only its *extent* rule is the loose one.) Kept so the
    /// M16 self-test can drive the *same* crafted image through both readers and
    /// watch the flag leak here and be contained by `mount`. This is a teaching
    /// device (like M15's `SYS_WRITE_UNCHECKED`); it must never be the reader a
    /// real system trusts. The one-line difference from `mount` — the extent
    /// ceiling — *is* the milestone.
    pub fn mount_loose(image: &'a [u8]) -> Result<Fs<'a>, FsError> {
        Self::mount_inner(image, false)
    }

    /// Shared parse for [`mount`](Fs::mount) (`confine = true`) and
    /// [`mount_loose`](Fs::mount_loose) (`confine = false`). The only behavioural
    /// difference is whether a file extent is ceilinged by `data_end` or by the
    /// image length — everything else (magic, version, sizes, the directory tree's
    /// disjoint/forward/depth validation) is identical.
    fn mount_inner(image: &'a [u8], confine: bool) -> Result<Fs<'a>, FsError> {
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
        // The data-region ceiling (v3). It must lie within the image and past the
        // superblock; a malformed value is rejected before it's trusted as a bound.
        // (A valid-but-forged value is not caught — see `mount`'s caveat.)
        let data_end = read_u32_le(image, SB_DATA_END) as usize;
        if data_end < SUPERBLOCK_LEN || data_end > image.len() {
            return Err(FsError::BadDataEnd);
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
        // entry's extent is checked in-bounds; every subdirectory is recursed into,
        // but only if its table starts at/after `high_water` (the end of every
        // table seen so far). That single monotone invariant makes tables globally
        // disjoint and forward-ordered, so validation is linear and can't loop,
        // blow up exponentially on a DAG, or cycle; a separate `MAX_DEPTH` cap keeps
        // a deep chain from overflowing the stack. See `validate_dir`.
        //
        // M16: when `confine`, a FILE extent must also end at/before `data_end` —
        // closing the gap where a crafted entry aliases the reserved bytes (the
        // secret). The floor (a file may still alias metadata or another file's
        // bytes below `data_end`) is left as a documented red-team/fuzzing target.
        let mut high_water = SUPERBLOCK_LEN;
        let ceiling = if confine { data_end } else { image.len() };
        validate_dir(image, SUPERBLOCK_LEN, dir_bytes, &mut high_water, 0, ceiling)?;

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

/// Deepest directory nesting `validate_dir` will follow. Real ZranFS trees are
/// 1–2 deep; this cap keeps a crafted deep chain from overflowing the kernel
/// stack (task stacks are 16 KiB with no guard page — see `task.rs`).
const MAX_DEPTH: usize = 32;

/// Recursively validate one directory region `[off, off + len)`: every entry's
/// extent must lie in the image, and every subdirectory must begin at/after
/// `*high_water` — the highest directory-table end seen so far — before we recurse
/// into it, after which `*high_water` advances past this region.
///
/// **Two guarantees, two mechanisms.**
///  - *Bounded work (no loop, no exponential blow-up).* `high_water` is monotone,
///    so every directory table must start strictly after every table already
///    validated: tables are globally disjoint and strictly forward-ordered. Each
///    is therefore validated **exactly once** (a second entry pointing at an
///    already-seen table — a cycle, back-edge, or a "diamond" DAG — has
///    `off < *high_water` and trips `DirNotForward`), so total work is
///    `O(image.len() / DIRENT_LEN)`, never exponential.
///  - *Bounded depth (no stack overflow).* Global forward-ordering bounds depth by
///    `image.len() / DIRENT_LEN`, which can still be tens of thousands for a large
///    image — far past a 16 KiB stack. So `depth` is capped at `MAX_DEPTH`
///    independently, turning a deep chain into a clean `TooDeep` error instead of
///    a silent stack overflow.
fn validate_dir(
    image: &[u8],
    off: usize,
    len: usize,
    high_water: &mut usize,
    depth: usize,
    extent_ceiling: usize,
) -> Result<(), FsError> {
    if depth > MAX_DEPTH {
        return Err(FsError::TooDeep); // a crafted deep chain — do not overflow the stack
    }
    if off < *high_water {
        return Err(FsError::DirNotForward); // cycle / back-edge / superblock / re-seen table (DAG)
    }
    if len % DIRENT_LEN != 0 {
        return Err(FsError::BadDirLength); // not a whole array of entries
    }
    let end = off.checked_add(len).ok_or(FsError::EntryOutOfBounds)?;
    if end > image.len() {
        return Err(FsError::EntryOutOfBounds);
    }
    *high_water = (*high_water).max(end); // this table is now consumed; nothing may reuse it

    for i in 0..(len / DIRENT_LEN) {
        let base = off + i * DIRENT_LEN;
        let c_off = read_u32_le(image, base + ENT_OFFSET) as usize;
        let c_len = read_u32_le(image, base + ENT_LENGTH) as usize;
        let c_end = c_off.checked_add(c_len).ok_or(FsError::EntryOutOfBounds)?;
        if c_end > image.len() {
            return Err(FsError::EntryOutOfBounds);
        }
        // M16: confine *every* entry's extent to the data region — under the strict
        // `mount`, `extent_ceiling` is `data_end`, so any extent reaching into the
        // reserved `[data_end, total)` bytes (where the secret lives) is rejected
        // here. Under `mount_loose`, `extent_ceiling` is the image length, so this is
        // a no-op (already guaranteed above) — the M11 behaviour, and the leak.
        //
        // Applied to directories too, not just files: a `KIND_DIR` table also may
        // not sit past `data_end`. That closes the scope asymmetry the M16 red-team
        // flagged (a dir table in the reserved region was un-exploitable here only by
        // the accident that the 25-byte secret doesn't parse as 32-byte entries — an
        // accident of the data, not a check). Legit tables live well below `data_end`,
        // so this costs a valid image nothing.
        if c_end > extent_ceiling {
            return Err(FsError::ExtentEscapesData);
        }
        match read_u32_le(image, base + ENT_KIND) {
            KIND_FILE => {} // extent already confined above
            KIND_DIR => validate_dir(image, c_off, c_len, high_water, depth + 1, extent_ceiling)?,
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

/// Build the boot RAM disk: a valid ZranFS **v3 tree**. This is the *writer* — it
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
        ("shell.txt", b"cd, pwd, ls, cat -- navigation over ZranFS v3.\n"),
    ];
    // The planted secret: bytes with no directory entry pointing at them (§8).
    // `self_test`'s M16 attack independently re-declares this exact literal (writer
    // and test share no types); the two MUST stay byte-identical, or the crafted
    // extent length / byte-equality assert there fails and boot panics.
    const SECRET: &[u8] = b"FLAG{ziran-boundary-leak}";

    let root_count = root_files.len() + 1; // + the docs/ directory entry
    let docs_count = docs_files.len();

    let root_table_off = SUPERBLOCK_LEN;
    let docs_table_off = root_table_off + root_count * DIRENT_LEN;
    let data_off = docs_table_off + docs_count * DIRENT_LEN;

    let data_total: usize = root_files.iter().map(|(_, b)| b.len()).sum::<usize>()
        + docs_files.iter().map(|(_, b)| b.len()).sum::<usize>();
    // `data_end` is where addressable file data stops and the reserved region (the
    // secret) begins. The strict `mount` confines every file extent to `<= data_end`,
    // so the secret in `[data_end, total)` is unreachable through any entry.
    let data_end = data_off + data_total;
    let total = data_end + SECRET.len();

    let mut img = Vec::with_capacity(total);

    // Superblock (v3: the reserved word is now `data_end`).
    img.extend_from_slice(&MAGIC);
    img.extend_from_slice(&VERSION.to_le_bytes());
    img.extend_from_slice(&(root_count as u16).to_le_bytes());
    img.extend_from_slice(&(total as u32).to_le_bytes());
    img.extend_from_slice(&(data_end as u32).to_le_bytes()); // 0x0C data_end (v3)

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
/// confirm the planted secret is unreachable by navigation, reject the
/// deliberately-corrupt images (a directory *cycle* among them, without looping),
/// and — the Milestone 16 payload — **capture the hidden flag** through the loose
/// reader and watch the strict reader (`data_end` confinement) contain it. A reader
/// that returns the right bytes on a good image, refuses every malformation, and
/// leaks the secret only when its extent check is loosened is provably parsing —
/// and the flag capture makes the boundary a thing you can feel, not assert.
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

    // A forward chain of directories deeper than MAX_DEPTH must be rejected
    // (`TooDeep`) rather than recurse until the stack overflows.
    let deep = {
        let levels = MAX_DEPTH + 2;
        let total = SUPERBLOCK_LEN + levels * DIRENT_LEN + 1;
        let mut img = Vec::with_capacity(total);
        img.extend_from_slice(&MAGIC);
        img.extend_from_slice(&VERSION.to_le_bytes());
        img.extend_from_slice(&1u16.to_le_bytes()); // root has 1 entry
        img.extend_from_slice(&(total as u32).to_le_bytes());
        img.extend_from_slice(&(total as u32).to_le_bytes()); // data_end = total (no reserved region); TooDeep trips first
        for k in 0..levels {
            if k == levels - 1 {
                write_entry(&mut img, "d", (total - 1) as u32, 1, KIND_FILE); // tail: a 1-byte file
            } else {
                let next = SUPERBLOCK_LEN + (k + 1) * DIRENT_LEN;
                write_entry(&mut img, "d", next as u32, DIRENT_LEN as u32, KIND_DIR);
            }
        }
        img.push(0u8);
        img
    };
    assert_eq!(Fs::mount(&deep).err(), Some(FsError::TooDeep));

    // A "diamond": two root entries pointing at the *same* child table. The
    // high-water rule rejects the second (`DirNotForward`), so mount validates
    // each table once — no exponential re-validation, no hang.
    let diamond = {
        let child = SUPERBLOCK_LEN + 2 * DIRENT_LEN; // 80
        let data = child + 2 * DIRENT_LEN; // 144
        let total = data + 1;
        let mut img = Vec::with_capacity(total);
        img.extend_from_slice(&MAGIC);
        img.extend_from_slice(&VERSION.to_le_bytes());
        img.extend_from_slice(&2u16.to_le_bytes()); // root has 2 entries
        img.extend_from_slice(&(total as u32).to_le_bytes());
        img.extend_from_slice(&(total as u32).to_le_bytes()); // data_end = total; DirNotForward trips first
        write_entry(&mut img, "a", child as u32, (2 * DIRENT_LEN) as u32, KIND_DIR);
        write_entry(&mut img, "b", child as u32, (2 * DIRENT_LEN) as u32, KIND_DIR);
        write_entry(&mut img, "x", data as u32, 1, KIND_FILE);
        write_entry(&mut img, "y", data as u32, 1, KIND_FILE);
        img.push(0u8);
        img
    };
    assert_eq!(Fs::mount(&diamond).err(), Some(FsError::DirNotForward));

    crate::serial_println!(
        "[ok] fs: mounted a v3 tree ({} root entries incl. docs/), read /docs/filesystem.txt \
         byte-for-byte, secret unreachable by navigation, 10 corrupt images rejected (cycle, deep \
         chain, and diamond DAG among them -- no hang, no stack overflow)",
        root.len()
    );
    crate::serial_println!("M11: filesystem online");

    // --- Milestone 16: break the filesystem boundary (flag capture) ---
    // The planted secret lives in [data_end, total), past the addressable data
    // region. Navigation can't reach it (asserted above). But a crafted *file
    // entry* whose extent aliases those bytes is the exfil path: the loose reader
    // (M11's in-image-only check) hands them back; the strict reader confines the
    // extent to data_end and rejects it. Same crafted image, one check apart. The
    // writer and this test independently know the flag (they share no types).
    const SECRET: &[u8] = b"FLAG{ziran-boundary-leak}";
    let data_end = read_u32_le(&image, SB_DATA_END) as usize;
    let e0 = SUPERBLOCK_LEN; // root entry 0's base offset

    // Craft the attack: repoint root entry 0 to a file named "leak" whose extent is
    // exactly the secret's byte range [data_end, data_end + SECRET.len()) == the tail.
    let mut craft = image.clone();
    let mut namebuf = [0u8; NAME_LEN];
    namebuf[..4].copy_from_slice(b"leak");
    craft[e0..e0 + NAME_LEN].copy_from_slice(&namebuf);
    craft[e0 + ENT_OFFSET..e0 + ENT_OFFSET + 4].copy_from_slice(&(data_end as u32).to_le_bytes());
    craft[e0 + ENT_LENGTH..e0 + ENT_LENGTH + 4].copy_from_slice(&(SECRET.len() as u32).to_le_bytes());
    craft[e0 + ENT_KIND..e0 + ENT_KIND + 4].copy_from_slice(&KIND_FILE.to_le_bytes());

    // (2) LEAK: the loose reader accepts the aliasing extent; read hands back the
    //     secret. Assert byte-equality with the planted flag — a real capture.
    let leaked_fs = Fs::mount_loose(&craft).expect("m16: loose mount should accept the crafted image");
    let leaked = leaked_fs.read_path("/leak").expect("m16: /leak unreadable under loose mount");
    assert_eq!(leaked, SECRET, "m16: the loose reader must leak the exact planted flag");
    crate::serial_println!(
        "[m16] CAPTURED: a crafted file extent aliased the secret -> loose mount leaked {:?}",
        core::str::from_utf8(leaked).unwrap_or("<non-utf8>")
    );

    // (3) CONTAINED: the strict reader confines the same extent to data_end.
    assert_eq!(
        Fs::mount(&craft).err(),
        Some(FsError::ExtentEscapesData),
        "m16: the strict reader must reject the extent that escapes the data region"
    );
    crate::serial_println!(
        "[m16] CONTAINED: strict mount rejected the escaping extent (ExtentEscapesData) -- flag safe"
    );

    // Boundary: an extent ending exactly at data_end is legal; one byte past is not.
    let mut edge_ok = image.clone();
    edge_ok[e0 + ENT_OFFSET..e0 + ENT_OFFSET + 4].copy_from_slice(&((data_end - 1) as u32).to_le_bytes());
    edge_ok[e0 + ENT_LENGTH..e0 + ENT_LENGTH + 4].copy_from_slice(&1u32.to_le_bytes());
    assert!(Fs::mount(&edge_ok).is_ok(), "m16: an extent ending exactly at data_end must pass");
    let mut edge_bad = image.clone();
    edge_bad[e0 + ENT_OFFSET..e0 + ENT_OFFSET + 4].copy_from_slice(&(data_end as u32).to_le_bytes());
    edge_bad[e0 + ENT_LENGTH..e0 + ENT_LENGTH + 4].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(
        Fs::mount(&edge_bad).err(),
        Some(FsError::ExtentEscapesData),
        "m16: one byte past data_end must be rejected"
    );

    // A malformed data_end (past total_size) is caught before it's trusted as a bound.
    let mut bad_de = image.clone();
    bad_de[SB_DATA_END..SB_DATA_END + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert_eq!(Fs::mount(&bad_de).err(), Some(FsError::BadDataEnd));

    // The confinement covers *directory* tables too, not just file extents — the
    // red-team's scope-asymmetry finding, now a pinned invariant. A synthetic image
    // whose one root entry is a DIR whose table sits past `data_end` must be rejected
    // (the boot image's 25-byte secret is too small to host a 32-byte table past
    // data_end, so this needs a bespoke image to exercise the KIND_DIR ceiling).
    let dir_escape = {
        let total = SUPERBLOCK_LEN + 2 * DIRENT_LEN; // superblock + root table + a child-table region
        let child_off = SUPERBLOCK_LEN + DIRENT_LEN; // 48
        let mut img = Vec::with_capacity(total);
        img.extend_from_slice(&MAGIC);
        img.extend_from_slice(&VERSION.to_le_bytes());
        img.extend_from_slice(&1u16.to_le_bytes()); // root: one entry
        img.extend_from_slice(&(total as u32).to_le_bytes());
        img.extend_from_slice(&(child_off as u32).to_le_bytes()); // data_end = 48: the child table lies past it
        write_entry(&mut img, "d", child_off as u32, DIRENT_LEN as u32, KIND_DIR);
        img.resize(total, 0); // the child-table region — in-image, but past data_end
        img
    };
    assert_eq!(
        Fs::mount(&dir_escape).err(),
        Some(FsError::ExtentEscapesData),
        "m16: a directory table past data_end must be rejected too, not only file extents"
    );

    // (4) PERMITS: the legitimate image still mounts strict and reads byte-exact
    //     (the happy path at the top used the strict Fs::mount) — confinement, not
    //     a blanket denial, is what makes the containment above meaningful.
    crate::serial_println!(
        "[m16] PERMITS: the legit boot image mounts strict and reads byte-exact -- a check, not a wall"
    );
    crate::serial_println!(
        "M16: filesystem boundary -- flag leaked by the loose reader, contained by data_end confinement"
    );
}
