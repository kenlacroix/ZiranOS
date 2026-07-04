# The filesystem, from first principles

*Companion to Milestone 11. Read this before `src/fs.rs` — it explains what that
file is *for* and why "just add a filesystem" hides the one idea the milestone is
actually about, which is not storage at all but a **lie**: a "file" is a story a
small header tells about a flat run of bytes, and a filesystem is the machinery
that tells the story consistently and — the harder half — refuses to tell it when
the bytes don't check out. `src/fs.rs` is the whole of it: a writer
([`boot_image`](../../src/fs.rs)) that packs a fixed set of files into a
`Vec<u8>`, and a reader ([`Fs::mount`](../../src/fs.rs)) that re-derives the
directory from those raw bytes and hands back `list`/`read`. This builds directly
on two siblings: [heap.md](heap.md) — the "disk" is a heap `Vec<u8>`, and this doc
borrows heap.md's habit of naming its own limitations out loud; and
[input-and-shell.md](input-and-shell.md) — the shell whose flat `match` this
milestone grows an `ls` and a `cat` onto, so the filesystem finally gives that
shell something to operate *on*. The one thing you need coming in: after the
earlier milestones we are in long mode with the heap up, so there is a `Vec<u8>`
to be a disk. Nothing here is `unsafe`, and nothing here touches hardware — this
is the first milestone that is pure data structure over memory the others built.*

---

## The problem: the shell can run commands, but the kernel has no notion of a "file"

Look at where M10 left us. The shell reads a line, parses a verb, and dispatches:
`help`, `echo`, `clear`, `mem`, `ps`. It can compute and it can be told things.
But run your eye down that command list and notice what is missing — there is
nothing the shell can *look at*. The kernel manages frames, pages, a heap, tasks,
and keystrokes, and every one of those is invisible: state with no name you can
type. The machine has plenty of bytes. What it does not have is a single byte you
can refer to as `motd.txt`.

That gap is smaller and stranger than "we need storage" makes it sound. We are
not short on a place to keep bytes — the heap has a mapped megabyte sitting right
there. What we are short on is a *naming scheme*. Bytes in memory are anonymous:
address `0x4000_0070` holds `0x48`, and nothing about that byte says it is the
first character of a file called `motd.txt`. So the real question underneath "add
a filesystem" is:

> **What, exactly, turns a flat, anonymous run of bytes into a set of named files
> you can `ls` and `cat`?**

The answer is deflating in the best way, and it is the whole milestone in one
sentence:

> **A "file" is a lie a header tells about a `&[u8]`.** A small table maps
> `{name → offset, length}`, and *that table is all that separates a byte array
> from a filesystem.* Give the bytes `[0x70..0x7d]` the name `motd.txt` and a
> length of 13, and you have a file — not because anything about those bytes
> changed, but because something now *claims* they are one.

Everything below is that sentence unpacked: the shape of the lie (the ZranFS
layout), who tells it (`boot_image`), who has to decide whether to believe it
(`Fs::mount`), and why deciding *not* to believe a malformed lie is the part that
actually took the engineering.

## A file is a lie — and the round-trip is what makes it honest

The design that makes this milestone teach cleanly is a deliberate *separation*
into two halves that share no code and no data type, only an agreement about bytes.

**The writer.** `boot_image()` is a tiny in-kernel `mkfs` — the thing that, on a
real system, formats a disk. It takes a fixed list of files and lays down a
`Vec<u8>` byte by byte: the superblock, then a directory entry per file, then the
file contents concatenated. Every multi-byte number is written little-endian with
`.to_le_bytes()`. It returns raw bytes and nothing else — no struct, no handle, no
metadata object.

```rust
const FILES: &[(&str, &[u8])] = &[
    ("motd.txt", b"Hello, Ziran!"),
    ("readme", b"Ziran OS -- a from-scratch x86_64 kernel.\n"),
    ("ziran.txt", "\u{81ea}\u{7136} (ziran): that which arises without external forcing.\n".as_bytes()),
];
```

**The reader.** `Fs::mount(image: &[u8])` takes those raw bytes — *just the
`&[u8]`* — and re-derives the entire directory from them. It reads the magic out
of the first four bytes, the file count out of bytes `0x06..0x08`, each entry's
name and offset and length out of the directory region, and it trusts **nothing
but the array.** There is no `struct Superblock` that both sides `#[repr(C)]`
their way through; the writer emits bytes and the reader parses bytes, and the
only thing joining them is the layout they both agree on.

This separation looks like extra work — why not share one packed struct and skip
the parsing? — but it is the entire point, and it is what makes the verification
later actually mean something. Because the reader never sees the writer's objects,
a passing round-trip proves the reader *decoded the layout*, not that it handed
its own struct back to itself. It is the difference between a translator who reads
the foreign text and one who was slipped the answer. Hold onto that; the
verification section cashes it in.

The whole data flow is two arrows and a gate:

```text
  boot_image()                 Fs::mount(&image)                 list() / read()
  (writer / mkfs)              (the trust boundary)              (total, panic-free)
        │                             │                                 │
   Vec<u8>: LE bytes  ──raw &[u8]──▶  validate every claim  ──ok──▶  safe slicing
   superblock+dir+data            (else Err(FsError))              name → &[u8]
```

## The ZranFS layout, byte by byte

ZranFS v1 has three regions, and the constants that name them live at the top of
`src/fs.rs`:

```rust
const MAGIC: [u8; 4] = *b"ZRFS";
const VERSION: u16 = 1;
const SUPERBLOCK_LEN: usize = 16;   // the directory table starts right after this
const DIRENT_LEN: usize = 32;       // one directory entry
const NAME_LEN: usize = 20;         // filename field width, NUL-padded

const ENT_OFFSET: usize = 0x14;     // u32 within an entry: byte offset of the file's data
const ENT_LENGTH: usize = 0x18;     // u32 within an entry: file length in bytes
```

**The superblock — 16 bytes at offset 0.** The disk's header: who it is, and how
big it says it is.

```text
  offset  size  field         value in our image
  0x00     4    magic         "ZRFS"  (5a 52 46 53)   — the signature mount checks first
  0x04     2    version       1                        — the format version this reader knows
  0x06     2    file_count    N                        — how many directory entries follow
  0x08     4    total_size    image length             — cross-checked against the real length
  0x0C     4    reserved      0                        — kept for the FAT-BPB teaching contrast
```

The one field doing real defensive work is `total_size`. It is *redundant* — a
reader already knows the image length; it was handed the slice. That is exactly
why it earns its bytes: a header that restates a fact you can independently check
is a header you can catch lying. If `total_size` disagrees with `image.len()`,
the disk's own story about its size doesn't match the disk, and `mount` refuses it
with `SizeMismatch`.

**The directory table — `DIRENT_LEN * file_count` bytes, starting at offset 16.**
This is the lie itself: the table that assigns names to extents. Each entry is 32
bytes.

```text
  +0x00   20    name          ASCII, NUL-padded (NAME_LEN = 20)
  +0x14    4    offset        byte offset from image start to this file's data
  +0x18    4    length        file length in bytes
  +0x1C    4    reserved      0
```

A file *is* nothing more than one of these rows: a name, and a `{offset, length}`
extent that points somewhere into the flat bytes. `read("motd.txt")` is, at
bottom, "find the row whose name is `motd.txt`, then return
`image[offset..offset+length]`." That slice is the file. There is no other secret.

**The data region — everything from offset `16 + 32*file_count` onward.** The file
contents, concatenated with no separators, no padding, no order requirement beyond
what the offsets say. A file's bytes are wherever its entry claims they are; the
region is just the raw material the directory carves names out of.

That base — `SUPERBLOCK_LEN + file_count * DIRENT_LEN` — is the one arithmetic the
whole format hinges on. `boot_image` computes it as `data_start` when it writes;
`mount` recomputes it as `dir_end` when it reads; both derive it from the same two
constants and the same `file_count`, so they can't drift.

## An actual image, annotated

Here is the payoff of the milestone: the real bytes `boot_image()` produces, for
the three files above, dumped and labeled field by field. This is not a mock-up —
it is what you would see if you hex-dumped the `Vec<u8>` the kernel builds at boot.
Read it against the layout tables above and the whole format stops being abstract.

```text
        0  1  2  3  4  5  6  7  8  9  a  b  c  d  e  f   ascii
0000   5a 52 46 53 01 00 03 00 e3 00 00 00 00 00 00 00   ZRFS............   ← SUPERBLOCK
0010   6d 6f 74 64 2e 74 78 74 00 00 00 00 00 00 00 00   motd.txt........   ┐
0020   00 00 00 00 70 00 00 00 0d 00 00 00 00 00 00 00   ....p...........   ┘ dir entry 0
0030   72 65 61 64 6d 65 00 00 00 00 00 00 00 00 00 00   readme..........   ┐
0040   00 00 00 00 7d 00 00 00 2a 00 00 00 00 00 00 00   ....}...*.......   ┘ dir entry 1
0050   7a 69 72 61 6e 2e 74 78 74 00 00 00 00 00 00 00   ziran.txt.......   ┐
0060   00 00 00 00 a7 00 00 00 3c 00 00 00 00 00 00 00   ........<.......   ┘ dir entry 2
0070   48 65 6c 6c 6f 2c 20 5a 69 72 61 6e 21 5a 69 72   Hello, Ziran!Zir   ← DATA begins
0080   61 6e 20 4f 53 20 2d 2d 20 61 20 66 72 6f 6d 2d   an OS -- a from-
0090   73 63 72 61 74 63 68 20 78 38 36 5f 36 34 20 6b   scratch x86_64 k
00a0   65 72 6e 65 6c 2e 0a e8 87 aa e7 84 b6 20 28 7a   ernel........ (z
00b0   69 72 61 6e 29 3a 20 74 68 61 74 20 77 68 69 63   iran): that whic
00c0   68 20 61 72 69 73 65 73 20 77 69 74 68 6f 75 74   h arises without
00d0   20 65 78 74 65 72 6e 61 6c 20 66 6f 72 63 69 6e    external forcin
00e0   67 2e 0a                                          g..
```

Walk it field by field:

- **`0x00`: `5a 52 46 53`** — ASCII `ZRFS`, the magic. `mount`'s first check reads
  exactly these four bytes.
- **`0x04`: `01 00`** — `version`, a little-endian `u16`. Low byte first: `0x0001`
  = 1. This is where little-endian byte order stops being trivia and starts being
  the substance — read these two bytes in the wrong order and you get `0x0100` =
  256, a version this reader rejects.
- **`0x06`: `03 00`** — `file_count` = 3. Three directory entries follow.
- **`0x08`: `e3 00 00 00`** — `total_size` as a little-endian `u32` = `0x0000_00e3`
  = 227. And the image really is 227 bytes long (the last byte is at `0xe2`). The
  story matches the disk, so `SizeMismatch` doesn't fire.
- **`0x0c`: `00 00 00 00`** — reserved.
- **`0x10`–`0x23`: dir entry 0.** The name field is `6d 6f 74 64 2e 74 78 74`
  (`motd.txt`, 8 bytes) followed by twelve `00` bytes of NUL padding — the field
  is always `NAME_LEN` = 20 wide regardless of the name's length. At `0x24`
  (`base + ENT_OFFSET`) the offset `70 00 00 00` = `0x70` = 112, and at `0x28`
  (`base + ENT_LENGTH`) the length `0d 00 00 00` = 13. So: *"the file `motd.txt`
  is the 13 bytes starting at 0x70."*
- **`0x30`–`0x53`: dir entries 1 and 2.** `readme`, offset `0x7d` = 125, length
  `0x2a` = 42; then `ziran.txt`, offset `0xa7` = 167, length `0x3c` = 60. Notice
  the offsets chain: `0x70 + 13 = 0x7d`, `0x7d + 42 = 0xa7` — each file's data
  begins exactly where the previous one ended, because `boot_image` lays them down
  contiguously.
- **`0x70` onward: the data region.** At `0x70`, `48 65 6c 6c 6f 2c 20 5a 69 72 61
  6e 21` — thirteen bytes spelling `Hello, Ziran!`. That is `motd.txt`, and
  `read("motd.txt")` returns precisely this slice, `image[0x70..0x7d]`. The bytes
  never announce which file they belong to; entry 0 *claims* them, and that claim
  is the entire file.

The data region begins at `0x70` = 112 = `16 + 32*3`, the `data_start`
computation, and every file's data is right where its offset promised. If you ever
suspect a parsing bug, this dump is the ground truth — read the bytes against the
format and the disagreement will be obvious.

## `mount()`: the whole trust boundary

Everything the reader does splits cleanly in two. `mount` is where *all* the doubt
lives; `list` and `read` live on the other side of it, where doubt has already
been spent and every access is known to be safe. Get `mount` right and the rest is
slicing.

`mount` validates in a specific order, each check mapping to exactly one
`FsError`:

```rust
pub fn mount(image: &'a [u8]) -> Result<Fs<'a>, FsError> {
    if image.len() < SUPERBLOCK_LEN {
        return Err(FsError::Truncated);            // (1) big enough for a header?
    }
    if image[..4] != MAGIC {
        return Err(FsError::BadMagic);             // (2) is it even a ZranFS disk?
    }
    if read_u16_le(image, 0x04) != VERSION {
        return Err(FsError::UnsupportedVersion);   // (3) a version we understand?
    }
    if read_u32_le(image, 0x08) as usize != image.len() {
        return Err(FsError::SizeMismatch);         // (4) does its size story hold up?
    }
    let file_count = read_u16_le(image, 0x06) as usize;

    let dir_bytes = file_count.checked_mul(DIRENT_LEN).ok_or(FsError::Truncated)?;
    let dir_end = SUPERBLOCK_LEN.checked_add(dir_bytes).ok_or(FsError::Truncated)?;
    if dir_end > image.len() {
        return Err(FsError::Truncated);            // (5) does the dir table fit?
    }

    for i in 0..file_count {
        let base = SUPERBLOCK_LEN + i * DIRENT_LEN;
        let offset = read_u32_le(image, base + ENT_OFFSET) as usize;
        let length = read_u32_le(image, base + ENT_LENGTH) as usize;
        let end = offset.checked_add(length).ok_or(FsError::EntryOutOfBounds)?;
        if end > image.len() {
            return Err(FsError::EntryOutOfBounds); // (6) every extent in-bounds?
        }
    }

    Ok(Fs { image, file_count })
}
```

The order is not cosmetic — each step establishes the precondition the next one
relies on. You cannot read the magic until you know there are four bytes to read
(step 1 guarantees 16). You cannot trust `file_count` enough to iterate entries
until you know the directory table those entries live in actually fits inside the
image (step 5). By the time the loop reads an entry's `offset` and `length`
fields, step 5 has already proven those field bytes are in-bounds; the loop's own
job is only to check that the *extent they describe* is in-bounds too.

**The substance is the checked arithmetic.** Steps 5 and 6 are the milestone's
real teeth, and they are guarding against one specific, classic parser bug: an
attacker-controlled length field multiplied or added into an offset that *wraps*.
Suppose `file_count` is read as some enormous value. `file_count * DIRENT_LEN`
with ordinary `usize` multiplication could overflow and wrap around to a *small*
number — one that passes a naive `<= image.len()` test, letting the loop march
off the end of the directory table reading garbage as file entries. `checked_mul`
returns `None` on overflow instead of wrapping, and `ok_or(FsError::Truncated)`
turns that `None` into a clean rejection. The same guards the per-entry
`offset.checked_add(length)`: a `length` chosen so `offset + length` overflows
would otherwise pass the bounds check and then slice out of range. This is the
whole reason `Truncated` names both "too short" and "the integer-overflow case" in
its doc comment — they are the same failure, one honest and one adversarial.

**After `mount` succeeds, the reader is total.** `list` and `read` do no
validation at all — and they don't need to, because `mount` already proved every
bound they will touch:

```rust
pub fn read(&self, name: &str) -> Option<&'a [u8]> {
    let image = self.image;
    for i in 0..self.file_count {
        let base = SUPERBLOCK_LEN + i * DIRENT_LEN;
        if entry_name(&image[base..base + NAME_LEN]) == name {
            let offset = read_u32_le(image, base + ENT_OFFSET) as usize;
            let length = read_u32_le(image, base + ENT_LENGTH) as usize;
            return Some(&image[offset..offset + length]);   // in-bounds, proven by mount
        }
    }
    None
}
```

That `&image[offset..offset + length]` would panic on a bad extent — but no
extent that survived `mount` is bad, so it never does. This is the payoff of
concentrating all doubt in one place: `read` returns `Option` only to express
"no such name," never "the disk was malformed," because a malformed disk could
never have produced this `Fs` in the first place. (The name decode uses
`entry_name`, which trims at the first NUL — or takes all 20 bytes if the name
fills the field with no terminator — and `from_utf8_lossy` so a non-ASCII name
never panics. `read` returns the *first* match if names somehow repeat; the writer
never produces duplicates, but the reader refuses to assume it.)

## Why it is pure safe Rust — and why that is the interesting point

There is no `unsafe` anywhere in `src/fs.rs`. That is not an accident of a small
module; it is the sharpest teaching point in the milestone, and it is worth being
precise about *why*.

Split "out of bounds" into two very different things:

- **Memory-unsafe out-of-bounds** — reading or writing an address the program has
  no right to touch. This is the CVE-shaped hazard: buffer overruns, wild pointers,
  reading the byte *after* the array into some unrelated structure. It corrupts
  silently and can be turned into an exploit.
- **Logical out-of-bounds** — a header field that *claims* a file lives at offset
  `0xFFFF_FFFF` of a 227-byte image. The claim is nonsense, but noticing it is a
  matter of arithmetic, not memory safety.

A read-only filesystem parser's whole exposure is the *second* kind. The bytes are
all in one `Vec<u8>` we own; the danger is never that we lack permission to touch
an address, it is that the header lies about which bytes mean what. And here is the
thing safe Rust changes: a bad offset that slips past validation does not become a
memory-safety bug — it becomes a **panic**. `&image[offset..offset + length]` with
an out-of-range range is a bounds-checked slice; Rust checks it at runtime and
panics loudly rather than reading past the buffer. So the worst a validation *gap*
could do here is crash the kernel with a clear message — never read another
structure's bytes, never corrupt anything, never leak.

Contrast the same filesystem written in C. There, `memcpy(dst, image + offset,
length)` with a bad `offset` reads straight past the buffer into whatever memory
follows — no check, no panic, just silent corruption or an information leak. The
identical logical bug is a memory-safety CVE in C and, in safe Rust, at worst a
loud panic. `mount`'s checked arithmetic upgrades even that panic into a graceful
`Err(EntryOutOfBounds)` — but the floor underneath it, the guarantee that a missed
check *degrades to a crash rather than a compromise*, is the language's, for free.
This is the concrete version of why the project is written in Rust, made visible in
the one place it matters most: parsing untrusted bytes.

## `ls` and `cat`: the shell, sitting on the filesystem

The shell wiring is deliberately thin — the depth is all in `fs.rs`, and the
commands are just a rendering layer over `list` and `read`. `parse` grows two arms
(`"ls" => Command::Ls`, `"cat" => Command::Cat(...)`), and `dispatch` calls into
two small functions.

`cmd_ls` mounts the boot image *fresh*, lists it, and prints a name/size row per
file:

```rust
fn cmd_ls() {
    let image = crate::fs::boot_image();
    let fs = match crate::fs::Fs::mount(&image) {
        Ok(fs) => fs,
        Err(e) => { println!("ls: cannot mount filesystem: {e:?}"); return; }
    };
    let files = fs.list();
    if files.is_empty() { println!("(no files)"); return; }
    for entry in files {
        println!("  {:20} {} bytes", entry.name, entry.length);
    }
}
```

`cmd_cat` does the same mount, then `read`s one file and prints its bytes — through
`from_utf8_lossy`, so a file full of arbitrary bytes renders as replacement
characters instead of panicking:

```rust
match fs.read(name) {
    Some(bytes) => print!("{}", alloc::string::String::from_utf8_lossy(bytes)),
    None => println!("no such file: {name}"),
}
```

The one design choice worth calling out is **rebuild-per-command**. Both `cmd_ls`
and `cmd_cat` call `boot_image()` and `mount` every time, building the whole
`Vec<u8>` disk from scratch on each command. That sounds wasteful and is exactly
right for M11: the image is ~227 bytes, and no shared mutable static means no lock,
no lifetime gymnastics, nothing an interrupt could observe half-built. The "disk"
is stateless because a read-only filesystem *has* no state to keep — every mount
re-derives everything from the bytes, the same honesty the writer/reader split is
built on. If M12 wants a persistent handle it can cache the image behind a
`Mutex<Option<Vec<u8>>>` like `SCHED`; M11 doesn't need to.

## Verification honesty: working vs. accidentally working

The sibling docs are all careful about the gap between something that *works* and
something that only *looks* like it works, and a filesystem parser has a
particularly seductive version of the trap. The obvious test — "mount the image
`boot_image` just built, read `motd.txt`, check it says `Hello, Ziran!`" — can pass
for entirely the wrong reason. If the reader and writer shared a struct, the test
would prove only that an object survived a round-trip through its own serializer;
a completely broken layout parser could still hand its own bytes back.

This is exactly why the writer/reader separation from the top of this doc is not
architectural fussiness — it is what makes the round-trip test *mean* something.
`mount` never sees `boot_image`'s objects; it is handed a `&[u8]` and must decode
the ZranFS layout to get anything at all. So when `self_test` asserts the exact
bytes:

```rust
let fs = Fs::mount(&image).expect("fs: boot image failed to mount");
assert!(list.iter().any(|e| e.name == "motd.txt" && e.length == 13), "...");
let motd = fs.read("motd.txt").expect("fs: motd.txt unreadable");
assert_eq!(motd, &b"Hello, Ziran!"[..], "fs: motd.txt bytes wrong");
assert!(fs.read("nope").is_none(), "fs: a phantom file was readable");
```

it is proving the reader *decoded the offset-and-length layout correctly* — found
entry 0's name field, read its little-endian offset `0x70` and length `13`, and
sliced the right 13 bytes — not that it round-tripped an object. A wrong
`data_start`, a swapped offset/length, a byte-order mistake: each produces
plausible-but-wrong bytes here and fails the `assert_eq!`.

But reading a *good* image is the easy half, and the milestone is honest that it is
the easy half. **The interesting part of a read-only filesystem isn't reading — a
successful read is just slicing. It is refusing to read a lie that doesn't check
out.** The validation *is* the substance, so the self-test spends most of its
effort proving each check actually fires, feeding `mount` five deliberately-corrupt
images and asserting the *specific* error each one earns:

```rust
let mut bad_magic = image.clone();  bad_magic[0] = b'X';
assert_eq!(Fs::mount(&bad_magic).err(), Some(FsError::BadMagic));

let mut bad_version = image.clone(); bad_version[4] = 99;
assert_eq!(Fs::mount(&bad_version).err(), Some(FsError::UnsupportedVersion));

assert_eq!(Fs::mount(&image[..8]).err(), Some(FsError::Truncated));

let mut bad_size = image.clone(); bad_size[8] = bad_size[8].wrapping_add(1);
assert_eq!(Fs::mount(&bad_size).err(), Some(FsError::SizeMismatch));

let mut oob = image.clone();
let off_field = SUPERBLOCK_LEN + ENT_OFFSET;
oob[off_field..off_field + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
assert_eq!(Fs::mount(&oob).err(), Some(FsError::EntryOutOfBounds));
```

Each corruption is one byte (or one truncation) away from the valid image, and each
asserts not merely that `mount` failed but that it failed *for the right reason* —
`BadMagic` when the signature is wrong, `SizeMismatch` when `total_size` no longer
matches, `EntryOutOfBounds` when an offset points off the disk. A reader that
returns the right bytes on a good image *and* refuses five specific malformations,
each with its named error, is provably parsing the layout, not coincidentally
succeeding. When all of it passes, one line goes out the serial port:

```text
[ok] fs: mounted 3 files, motd.txt verified byte-for-byte, 5 corrupt images rejected
M11: filesystem online
```

That is the deterministic proof — no keyboard, no eyeballing. The interactive
`ls`/`cat` under `make run` are the demo; the five rejections on serial are the
argument. And they are the seed of something later: the M16 fuzzing track is
exactly "throw malformed images at `mount` and confirm it never panics, only
`Err`s." These five hand-written corruptions are the first five fuzz cases,
written by hand.

## The FAT16 contrast: the same lie, told with indirection

It is worth mapping ZranFS onto a *real* filesystem, because the exercise shows
that the lesson generalizes and that ZranFS's simplicity is a deliberate
subtraction, not naïveté. FAT16 — the format on every old floppy and early flash
card — tells the exact same lie ("a file is a name plus a map to its bytes"), just
with more indirection and a great deal of accidental complexity.

Line the structures up:

- **Our superblock ↔ FAT's BPB (BIOS Parameter Block).** Where ZranFS's superblock
  is 16 bytes with four meaningful fields, FAT16's BPB is a ~25-field header
  crammed into the first sector: bytes-per-sector at `0x0B`, sectors-per-cluster at
  `0x0D`, a count of reserved sectors, the number of FATs (usually two, redundant
  copies), the number of root-directory entries, sectors-per-FAT, and more. Most of
  those fields exist to describe a geometry — sectors, clusters, multiple
  allocation tables — that our RAM disk simply doesn't have. Same job (describe the
  disk), an order of magnitude more surface.
- **Our directory table ↔ FAT's root directory region.** This one is nearly a
  direct match: FAT16's root directory is an array of 32-byte entries, the same
  size as our `DIRENT_LEN`. Each holds an 8.3 name (eight name bytes, three
  extension bytes), an attributes byte at `0x0B`, the file's size at `0x1C`, and —
  the crucial difference — a *first-cluster* number at `0x1A` rather than a byte
  offset.
- **Our `{offset, length}` ↔ FAT's cluster chain.** This is where the lie gets its
  indirection. ZranFS says "the file's bytes are the contiguous run
  `[offset..offset+length]`" — one slice, done. FAT16 stores only the *first*
  cluster in the directory entry; to find the rest, you walk the **File Allocation
  Table**, a big array of `u16` "next cluster" links. Cluster 5's contents come
  from the data region, and `FAT[5]` tells you the *next* cluster, and `FAT[that]`
  the one after, following the chain until you hit an end-of-chain marker
  (`0xFFF8`–`0xFFFF`). A FAT16 file is a *linked list* threaded through a side
  table; our file is a slice. The linked list buys fragmentation — a file's bytes
  can be scattered across non-adjacent clusters — which is real and useful, and
  which we deliberately gave up.

The lie is identical; FAT just tells it with a level of indirection (cluster chains
instead of extents) and a pile of geometry (the BPB) that is *accidental* to the
lesson. Worse for a from-scratch teaching kernel, producing a *valid* FAT16 image
realistically means running `mkfs.fat` on the host and shipping the opaque blob it
spits out — precisely the "build magic that hides how bytes become data" the
project's PLAN §4 guardrail forbids. ZranFS keeps the whole round-trip in readable
Rust you can hex-dump against a table, which is why M11 chose it. Reading a real
FAT16 image — "parse a format you didn't design" — is a worthwhile future exercise,
but a deferred post, not this milestone.

## The honest limitations

Following the discipline the sibling docs insisted on — name what the design
*doesn't* do, out loud — ZranFS has a stack of deliberate omissions, each a
learning-scoped choice with a named path forward:

- **Read-only.** There is no `write`, no `create`, no `delete`. Writing needs
  free-space management (a way to find room for new bytes and reclaim freed ones) —
  the filesystem equivalent of the heap's free list, and a milestone's worth of
  work on its own. M11 reads; it does not modify.
- **A single flat directory.** Every file lives in one namespace with no
  subdirectories — there is no `/`, no folders, no paths. The directory is a flat
  array of entries. Nesting is an obvious M12 follow-on (a directory becomes just
  another file whose contents are more directory entries), deliberately not started
  here.
- **Contiguous file data, no blocks.** Each file is one unbroken run of bytes named
  by `{offset, length}`. There is no block or cluster layer and therefore no
  fragmentation — which is exactly the simplification the FAT16 contrast highlighted
  and gave up. Clean to teach; unable to grow a file in place.
- **No block-device abstraction; the "disk" is a heap `Vec`.** There is no driver,
  no sector interface, nothing between `mount` and the bytes — the disk is a
  `Vec<u8>`, rebuilt fresh on every `ls`/`cat` (see the stateless-by-design note
  above). A real system layers the filesystem over a block device; ZranFS layers it
  directly over memory, because a block device is a hardware milestone the project
  is not doing.
- **Extents are checked in-bounds, not confined to the data region.** `mount`
  verifies every entry's `offset + length` lies within the image, but *not* that
  `offset >= dir_end` or that extents don't overlap. So a crafted image could name
  the superblock or another file's bytes as a file's "contents." That is harmless
  here — every such slice is still in-bounds and read-only, so the worst outcome is
  a file that reads as gibberish — but tightening it into two more rejection cases
  (`offset` must land in the data region; extents mustn't alias) is a natural M16
  fuzzing-track addition. The code's own comment flags it.
- **Names cap at 20 bytes.** `NAME_LEN` is 20, NUL-padded; a longer name is
  silently truncated by the writer (`take = nb.len().min(NAME_LEN)`). Fine for a
  teaching disk with a handful of short names, a real ceiling nonetheless.

None of these corrupt memory — the pure-safe-Rust guarantee holds regardless.
They are all *absence of features*, not *presence of bugs*, and each is the natural
next thing a filesystem grows.

---

# Milestone 12: a directory is the same lie, recursively

*Companion to the M12 update of `src/fs.rs`. Everything above is ZranFS v1: one
flat directory, no folders, no paths. M12 is the capstone — the file manager, the
project's stated end goal ("list, navigate, and read files"). Read this after the
v1 material above; it assumes the layout tables, the trust-boundary framing, and
the writer/reader split, and extends all three by one idea.*

## The thesis: so is a directory

M11 landed on a single deflating sentence: **a file is a lie a header tells about a
`&[u8]`.** A row `{name, offset, length}` points at a run of flat bytes and
*claims* they are a file. Nothing about the bytes changed; something now says a
story about them.

M12 is that sentence applied to itself:

> **So is a directory.** A directory is just a file whose bytes are *more*
> `{name, offset, length}` entries. Point a row at a region and call that region a
> table of rows, and you have a folder — not because those bytes are special, but
> because something now claims they are a directory instead of a document. **The
> same lie, told recursively.**

That is the whole format delta and the whole conceptual move. A subdirectory is not
a new kind of thing; it is the existing thing (a 32-byte entry) pointing at a region
the reader agrees to parse as an array of the existing thing. The flat namespace
becomes a tree the instant one entry is allowed to say "my bytes are entries too."

## The format delta is exactly one field

ZranFS v2 changes v1 in two places, and one of them is just a number.

**The superblock is byte-identical** — only `version` now reads 2 (`VERSION: u16 =
2`). A v1 image mounts as `UnsupportedVersion`; we keep only the current reader.

**The directory entry changes one field.** M11 left a `u32` at `+0x1C` labeled
`reserved` — kept, the v1 doc said, "for the FAT-BPB teaching contrast." M12 spends
it:

```text
  +0x00   20    name          ASCII, NUL-padded (NAME_LEN = 20)      [unchanged]
  +0x14    4    offset        byte offset to this node's region      [unchanged]
  +0x18    4    length        region length in bytes                 [unchanged]
  +0x1C    4    kind          0 = file, 1 = directory                [was `reserved`]
```

The constants that name it (`ENT_KIND: usize = 0x1C`, `KIND_FILE: u32 = 0`,
`KIND_DIR: u32 = 1`) sit right beside v1's `ENT_OFFSET`/`ENT_LENGTH`. The rule the
`kind` byte selects between:

- A **FILE** entry's `[offset..offset+length)` is opaque file bytes — v1 behavior,
  unchanged.
- A **DIR** entry's `[offset..offset+length)` is an array of `length / DIRENT_LEN`
  child entries — the same 32-byte rows, recursively. `length % 32` must be 0;
  `length == 0` is a valid empty directory. The root table (at offset 16,
  `file_count` entries) *is* the top-level directory.

The boot image `boot_image()` builds is now a depth-2 tree:

```text
  /                            (root table, 4 entries)
  ├─ motd.txt      FILE   "Hello, Ziran!"
  ├─ readme        FILE   "Ziran OS -- a from-scratch x86_64 kernel.\n"
  ├─ ziran.txt     FILE   the 自然 line (unchanged from v1)
  └─ docs/         DIR ───┐  (points at the docs child table)
                          ├─ filesystem.txt  FILE  "A file is a lie a header tells about bytes.\n"
                          └─ shell.txt        FILE  "cd, pwd, ls, cat -- navigation over ZranFS v2.\n"

  [hidden]  b"FLAG{ziran-boundary-leak}"   bytes in the image with NO entry naming them
```

The physical layout still lays down as one contiguous `Vec<u8>`, and the order is
not arbitrary — it is what makes the tree *provably* valid (next section):

```text
  superblock          [0x00 .. 0x10)     16 bytes
  root table          [0x10 .. 0x90)     4 entries × 32 = 128 bytes
  docs child table    [0x90 .. 0xB0)     2 entries × 32 = 64 bytes  (starts at root_end)
  file data           root files, then docs files, concatenated
  hidden secret       b"FLAG{ziran-boundary-leak}"  — last, named by nothing
```

Notice the `docs/` child table begins *exactly* where the root table ends (`0x90 =
16 + 4*32`). That is not a coincidence of packing; it is the invariant the writer
is careful to satisfy so the reader's cheapest validation strategy works. Hold that
thought — it is the centerpiece.

## The hard part: validating a tree you don't trust — without looping OR blowing up

Reading a good tree is easy: split the path on `/`, walk `DIR` entries down from the
root, slice. The milestone's real work — and its honest, transferable lesson — is
`mount` proving an **attacker-controlled** tree is safe to walk *before* anything
walks it. A malformed image can make a `DIR` entry's `offset` point back at its own
table, at an ancestor, or at the superblock: a cycle. A naive recursive validator
follows that cycle forever, and `mount` hangs — the boot never completes.

The tempting fix is a forward-ordering rule: a child table must start at/after its
parent's end (`c_off >= p_end`). Along any root-to-leaf descent, region start
offsets then strictly increase, and a strictly increasing sequence of integers all
`< image.len()` is finite. So the recursion **terminates**. Clean proof. Ship it?

No — and this is the whole lesson. **A termination proof is not a DoS-safety
proof.** "Terminates," "runs in bounded time," and "uses bounded stack" are three
*separate* guarantees, and the milestone learned the hard way that conflating them
is exactly how a validator ships a denial-of-service. Take them one at a time.

**1. Termination (bounded depth of *descent*).** The forward rule above gets you
this and only this: descent offsets strictly increase, so no path is infinite, so
`validate_dir` returns. Necessary. Nowhere near sufficient — "finite" says nothing
about *how* finite.

**2. Bounded work (no exponential blow-up).** Here is the trap the naive forward
rule walks straight into. Nothing in `c_off >= p_end` says two *sibling* entries
can't point at the **same** child region — a "diamond" DAG:

```text
      root
     ┌─┴─┐
     a   b          both a and b are DIR entries whose offset is the SAME table C
     └─┬─┘
       C            ... and C holds two DIRs pointing at the same table D ...
      ┌┴┐
      D D
```

Every descent still has strictly increasing offsets, so it *terminates* — but the
validator re-validates C once per path that reaches it, D once per path that reaches
*it*, and with `d` diamonds stacked the work is `2^d`. Terminates, yes; in
astronomically unbounded *time*. `mount` would hang for a different reason than the
cycle — not an infinite loop, an exponential one — and the boot still never
finishes. **Finite is not fast.**

The fix is to strengthen forward-ordering from *per-descent* to *global*: a
monotone **high-water mark**. `validate_dir` threads a `&mut usize` high_water — the
highest table-end seen anywhere so far — and every directory table must start at/
after it (`off < *high_water` → `DirNotForward`), after which high_water advances
past this table's end. That single monotone invariant makes *all* tables globally
disjoint and strictly forward-ordered, so each is validated **exactly once**. A
second entry pointing at an already-seen table — a cycle, a back-edge, or a diamond's
shared child — now has `off < *high_water` and trips `DirNotForward` immediately.
Total work drops from `2^depth` to `O(image.len() / 32)`: linear, one pass.

**3. Bounded stack (no overflow).** Even a *valid* image can defeat both proofs
above. Global forward-ordering bounds descent depth by `image.len() / 32` — which
for a large image is tens of thousands of frames. `validate_dir` recurses once per
level, and the shell task's stack is **16 KiB with no guard page** (see `task.rs`):
a chain that deep silently walks the recursion off the end of the stack and corrupts
whatever memory follows. No panic, no error — the guard page that would have caught
it doesn't exist. This is the quietest failure of the three, and neither termination
nor linear-time work says a word about it.

The fix is blunt and separate: a `MAX_DEPTH = 32` cap, checked at the top of
`validate_dir`, turning a too-deep chain into a clean `Err(TooDeep)`. Real ZranFS
trees are 1–2 deep; 32 is enormous headroom and still tens of thousands short of the
stack. It has to be its own mechanism precisely because it guards a resource
(stack frames) that the offset arithmetic never touches.

So `validate_dir` carries **three guards for three distinct claims**, and no two of
them are the same claim:

```rust
fn validate_dir(image, off, len, high_water: &mut usize, depth) -> Result<(), FsError> {
    if depth > MAX_DEPTH  { return Err(FsError::TooDeep); }       // bounded STACK
    if off < *high_water  { return Err(FsError::DirNotForward); } // bounded WORK (global disjoint)
    if len % DIRENT_LEN != 0 { return Err(FsError::BadDirLength); }
    let end = off.checked_add(len).ok_or(FsError::EntryOutOfBounds)?;
    if end > image.len()  { return Err(FsError::EntryOutOfBounds); }
    *high_water = (*high_water).max(end);                          // this table is now consumed
    for i in 0..(len / DIRENT_LEN) {
        // ... check each child extent in-bounds ...
        match kind {
            KIND_FILE => {}                                        // in-bounds is enough
            KIND_DIR  => validate_dir(image, c_off, c_len, high_water, depth + 1)?,
            _         => return Err(FsError::BadKind),
        }
    }
    Ok(())
}
```

The honest lede, stated plainly because it is the genuinely valuable thing to carry
out of this milestone: **a termination proof is not a DoS-safety proof.** When you
convince yourself a loop over untrusted input "can't run forever," you have proven
guarantee #1 and it is easy to believe you're done. You are not: #2 (does it run in
*bounded time*?) and #3 (does it run in *bounded stack*?) are different questions
with different answers, and a validator that stops at #1 ships a hang or a stack
smash to the first adversary who reads the code. Three bounds, three mechanisms,
three named errors — `DirNotForward`, `TooDeep`, and the in-bounds/`checked_add`
pair — because they are three different promises.

## Path resolution, split cleanly

Navigation needs `.` and `..`, and the naïve place to put them is the disk walk.
That would be a mistake: the flat format has **no parent pointers**. There is
nothing in a child table that points back at its parent — `..` is meaningless as a
disk operation. So M12 splits resolution into two halves that never touch each
other's job:

- **The shell owns a pure `canonicalize(cwd, arg) -> String`.** It joins `cwd` and
  `arg`, drops `.` and empty components, and handles `..` as a `Vec::pop` — string
  math, no disk. Popping past root is a no-op (`/` + `..` → `/`). The result is
  always a canonical absolute path: leading `/`, no `.`/`..`, no trailing slash
  except root itself. `..` is arithmetic on a path string, *never* a parent-pointer
  walk the format couldn't support.
- **The fs owns `resolve(abs: &str) -> Result<Node, FsError>`.** It takes an
  *already-canonical absolute path* and splits on `/`, walking `DIR` entries down
  from the root. Because the path is pre-canonicalized, `resolve` only ever
  **descends** — no `.`/`..`, no `cwd`, no cycles to chase. `Node` is `File{offset,
  length}` or `Dir{offset, length}`; descending through a `File` is `NotADirectory`,
  a missing component is `NotFound`.

The handoff is one clean line: **shell canonicalizes (string) → fs resolves (walk)
→ slice.** On top of `resolve`, the fs exposes `list_dir(abs)` (resolve to a `Dir`,
render its region as `DirEntry` rows with an `is_dir` bool) and `read_path(abs)`
(resolve to a `File`, return the byte slice; `IsADirectory` if it's a folder).

The shell grows `cd`, `pwd`, `ls`, and `cat`, threading a `cwd: String` that starts
at `"/"`, and a prompt that shows it:

```text
  ziran:/> cd docs
  ziran:/docs> ls
    filesystem.txt       43 bytes
    shell.txt            46 bytes
  ziran:/docs> cat filesystem.txt
  A file is a lie a header tells about bytes.
  ziran:/docs> cat .
  cat: is a directory
  ziran:/docs> cd ..
  ziran:/>
```

`cd` canonicalizes, confirms the target `is_dir` before mutating `cwd` (a failed
`cd` never moves you), and `ls`/`cat` with no arg operate on `cwd`.

## Verification honesty: mount a tree, then reject ten lies

The v1 self-test proved reading a good flat image and rejecting five malformations.
M12 keeps that discipline and extends it in exactly the two directions the new
format opened: *navigation* and *the two new denial-of-service shapes*.

The happy path now **navigates**: mount the tree, list `/`, confirm `docs/` is a
directory, descend into it, and read `/docs/filesystem.txt` byte-for-byte against
its literal. That last check is what proves the resolver *walked into a
subdirectory and decoded its child table* — not that it round-tripped an object, and
not that it accidentally read the right bytes from the root.

The rejection set grows from five to **ten**, and the three new mount-time cases are
precisely the three-bounds argument made executable:

- a **cycle** (a `DIR` whose offset points back at the root table) → `DirNotForward`,
  *without looping* — guarantee #1/#2;
- a **deep chain** deeper than `MAX_DEPTH` → `TooDeep`, *without overflowing the
  stack* — guarantee #3;
- a **diamond DAG** (two sibling entries pointing at the same child table) →
  `DirNotForward` on the second, proving each table is validated once, *without
  exponential blow-up* — guarantee #2;

plus `BadDirLength` (a dir length not a multiple of 32) and `BadKind` (a `kind`
that's neither 0 nor 1), and the v1 five carried forward. The whole point of the
cycle/deep/diamond trio is that a hang or a stack smash would **time out the boot**
— so a self-test that *completes* is itself the proof the guards fired. The serial
line says so out loud:

```text
[ok] fs: mounted a v2 tree (4 root entries incl. docs/), read /docs/filesystem.txt
     byte-for-byte, secret unreachable, 10 corrupt images rejected (cycle, deep
     chain, and diamond DAG among them -- no hang, no stack overflow)
M11: filesystem online
```

That "no hang, no stack overflow" is not decoration — it is the milestone claiming
all three bounds by name, and the fact that the line printed at all is the evidence.

## The hidden secret: the boundary holds against navigation, not against everything

`boot_image` plants one more thing in the image: the bytes
`b"FLAG{ziran-boundary-leak}"`, written last, **with no directory entry pointing at
them.** They are physically present in the `Vec<u8>` and completely unreachable by
`ls`/`cat` — because navigation can only follow entries, and no entry names the
secret. The self-test asserts this directly: `read_path("/FLAG")` and any other
guess return `NotFound`. **The boundary holds: what has no name cannot be
navigated to.**

But be precise about *which* boundary holds. Through Milestone 12, `mount` checked
that every file extent was *in-bounds* — `offset + length <= total_size` — but did
**not** confine it to the data region. A `FILE` entry could legally point its
`offset`/`length` at the superblock, at another file's bytes, or at the secret. So
while *navigation* could never reach the flag, a **crafted image** with a file entry
whose extent overlapped the secret read it right out. That was not an oversight; it
was the planted **M16 exfil target** (PLAN §8) — the secret was safe against
navigation, and deliberately *not* safe against an arbitrary crafted image.

**Milestone 16 is the adversary that builds that image — and then closes the gap.**
See "Milestone 16: the aliased extent" below: the fix is a **data-region ceiling**
(`data_end`, a v3 superblock field) that confines every entry's extent, so the same
crafted image now `mount`s to `ExtentEscapesData` instead of leaking. The lesson it
draws: *a bounds check is only as trustworthy as the bound it compares against* —
M11 bounded by `total_size` (the whole image); the fix bounds by `data_end` (where
the data ends).

## Milestone 16: the aliased extent (the confused deputy, one layer down)

The attack is one crafted directory entry. Take the boot image and repoint a
`FILE` entry so its `offset`/`length` cover the secret's byte range — for the
planted `FLAG{ziran-boundary-leak}`, `offset = data_end`, `length = 25`. Nothing
about the entry is malformed by M11's rules: the extent is in-bounds
(`offset + length == total_size`), the name is valid, the kind is a file. So
`mount` (through M12) accepts it, and `cat`-ing that entry returns the flag,
because `read_path` hands back exactly `&image[offset .. offset+length]`. The
filesystem was tricked into reading bytes on the caller's behalf that the caller
was never meant to reach — a **confused deputy** made of offsets instead of
pointers, the direct analogue of Milestone 15's syscall.

The fix names where the data actually ends. Format **v3** gives meaning to the
superblock's last reserved word: **`data_end`**, the offset where addressable file
data stops. The secret lives in `[data_end, total)`, past it. The strict reader
(`Fs::mount`) confines **every** entry's extent — file data *and* directory tables
— to end at or before `data_end`; an extent that reaches into the reserved region
is `ExtentEscapesData`. The M11 check compared against `total_size` (the whole
image); the fix compares against `data_end` (the legitimate data). Same crafted
image, different bound, opposite outcome.

The milestone ships both readers on purpose. `Fs::mount` is strict; `Fs::mount_loose`
keeps the old `total_size` bound, so the self-test drives the *same* crafted image
through both and you watch the flag leak through one and be contained by the other
— the "decision you can feel." (Like the ring-3 milestone's deliberately-unchecked
syscall, the loose reader is a teaching device, never a reader a real system trusts.)

Two honest residuals, both stated rather than hidden — because the point of the
security track is to know exactly how far a boundary extends:

- **A forged superblock.** `data_end` is itself a field *in the image*. An attacker
  who can rewrite the whole superblock (not just an entry) can set
  `data_end = total_size` and reopen the leak. `mount` range-checks `data_end`
  (`BadDataEnd` for out-of-range) but cannot make an in-image field trustworthy
  against a fully-forged header. This is Milestone 15's deepest lesson recurring:
  *validating against a value the caller controls is not validation.* Fully closing
  it needs structural coverage accounting or an out-of-band authority — out of scope
  for a teaching FS.
- **Floor-aliasing.** The fix is a ceiling, not a full "each byte owned by one
  file" accounting. A file extent *below* `data_end` may still overlap another
  file's bytes or the metadata — it can disclose structure, but never the secret
  (which lives above `data_end`). Named as a fuzzing target, not built.

The self-test was itself the acceptance test the fix was written against, and a
red-team pass hardened it: two independent reviewers both caught that the first fix
confined *file* extents but not *directory* tables — un-exploitable for this 25-byte
secret only by the accident that 25 isn't a multiple of the 32-byte entry size, so
the ceiling now applies to every entry uniformly.

# Where this goes next

Reaching M12 **completes the core arc** — boot → memory → tasks → shell → files.
The kernel can now list, navigate, and read files, which is the project's primary
stated success criterion; from here on the roadmap is stretch and security. The
filesystem gave the shell something to operate *on*, and the directory tree gave the
user somewhere to *go*.

- **Milestone 13 (userspace / syscalls)** introduces the first *privilege* boundary
  — ring 3 code that can't touch the kernel's memory directly and must ask for
  service through a syscall gate. Where the filesystem taught "refuse to believe a
  malformed lie about bytes," the syscall boundary teaches "refuse to act on a
  malformed request from a less-privileged caller" — the same trust-boundary
  discipline, one ring up.
- **The M15/M16 security track** turned this milestone's boundaries into targets,
  and both are now done. M15 captured a flag behind the ring-3 boundary through an
  unchecked syscall; **M16 captured `FLAG{ziran-boundary-leak}` through a crafted
  file extent and then closed the gap** with the `data_end` confinement (see
  "Milestone 16: the aliased extent" above). What remains here is the *generated*
  fuzzing flood — turning the dozen hand-written corrupt images into a machine-made
  stream against the strengthened `mount` contract (*never panic, never hang, never
  smash the stack; only `Ok` or a specific `Err`*) — plus the two named residuals
  (forged-superblock `data_end`, floor-aliasing) as sharper targets. A good future
  post, no longer a prerequisite for the capture.
- **A future FAT16 read post** still cashes in the v1 contrast: parse a format you
  didn't design, from a real `mkfs.fat` image, and meet the cluster chain and the
  BPB in the wild.

That is the shape of the capstone: teaching the kernel that a *directory* is not a
new thing but the *same claim, nested* — a header saying "the bytes over there are
themselves headers" — and that the craft is walking that nesting consistently while
refusing, in three distinct ways, to be hung, blown up, or crashed by a tree that
lies about its own shape. **A directory is a convincing lie about disk layout, told
recursively**, and `validate_dir` is the part that decides — in bounded depth,
bounded work, and bounded stack — whether to believe it.
