# Milestone 11 — read-only filesystem (ZranFS) — eng-plan

Locks the technical approach before building. Grounded in a two-way research
fan-out (the format/disk-source decision; the shell/build integration points).
Companion concept doc: `docs/concepts/filesystem.md` (written during the build).

The milestone teaches exactly one thing: **a "file" is a lie a header tells about
a flat run of bytes.** A *writer* packs little-endian bytes into a buffer; a
*reader* re-derives the whole directory from those raw bytes, trusting nothing but
the array. "Files are just convincing lies about disk layout" — the milestone is
understanding the lie.

## 0. Scope-guard (do this first)

Verdict **Hold**. The filesystem is where a teaching kernel is most tempted to
grow a real one. The decisions and deferrals:

- **Custom ZranFS, not FAT16.** FAT16 teaches a real format (résumé value), but
  most of its complexity — the ~25-field BPB, two redundant FATs, cluster-chain
  indirection, 8.3 names, the `0x00`/`0xE5`/`0xFFF8` sentinels — is *accidental*
  to the lesson, and a valid image realistically needs `mkfs.fat`: a host-tool
  dependency and an opaque prebuilt blob, exactly the "build magic that hides how
  bytes become data" the PLAN §4 guardrail forbids. Custom keeps the whole
  round-trip in readable Rust. **FAT16 is a deferred future post, not rejected.**
  This resolves PLAN §7's open question — recorded in PLAN.md so it isn't
  re-litigated.
- **In-kernel `mkfs` into a heap `Vec<u8>`, not `include_bytes!` or virtio-blk.**
  The "disk" is built at boot in readable Rust; no external image, no asset
  pipeline, no block-device driver (virtio-blk is the "real hardware" non-goal).
- **Read-only, flat namespace, contiguous data, exact-name lookup.** Defer
  writing (needs free-space management), subdirectories (an M12 follow-on), blocks
  / a block-device abstraction, caching / an open-file table, and a VFS layer
  (premature until there's a second backend).
- **No `unsafe`.** The FS is pure safe Rust — slice indexing over a heap `Vec`.
  The interesting hazard is *logical* out-of-bounds (a bad offset), not
  memory-unsafe out-of-bounds; `mount` validation turns the former into a clean
  `Err`, and safe Rust makes the latter a loud panic, never silent corruption.

## 1. Approach

A new `src/fs.rs` plus a handful of `ls`/`cat` arms in `src/shell.rs`. No asm, no
`unsafe`, no new hardware — this is the first milestone that is pure data
structure over the heap the earlier milestones built.

**The format — ZranFS v1** (all multi-byte fields little-endian, matching x86 and
the M6 Multiboot parser):

```
Superblock (16 bytes, at offset 0)
  0x00  4  magic       = "ZRFS" (5A 52 46 53)
  0x04  2  version     = 1
  0x06  2  file_count  = N
  0x08  4  total_size  = image length (redundant; a reader can cross-check)
  0x0C  4  reserved    = 0

Directory table (N × 32 bytes, at offset 16)
  +0x00 20  name    ASCII, NUL-padded/terminated
  +0x14  4  offset  byte offset from image start to this file's data
  +0x18  4  length  file length in bytes
  +0x1C  4  reserved = 0

Data region (at offset 16 + 32*N)
  file contents concatenated; each addressed only by its entry's {offset, length}
```

**The module.** `src/fs.rs`:
- `boot_image() -> Vec<u8>` — the in-kernel `mkfs`: packs a fixed set of files
  (`motd.txt` = `"Hello, Ziran!"`, `readme`, a `ziran.txt` with the 自然 line)
  into a valid ZranFS image on the heap. This is the *writer*; nothing it returns
  is trusted by the reader beyond the raw bytes.
- `Fs<'a> { image: &'a [u8], file_count: usize }` — a validated zero-copy view.
- `Fs::mount(image: &[u8]) -> Result<Fs, FsError>` — the entire trust boundary:
  validates magic, version, that the dir table fits, and that **every** entry's
  `offset + length` stays within the image (checked arithmetic). After a
  successful mount, `list`/`read` are total and panic-free.
- `Fs::list() -> Vec<DirEntry { name: String, offset, length }>` — what `ls` renders.
- `Fs::read(name: &str) -> Option<&[u8]>` — a slice straight into the image (no
  copy); what `cat` renders.
- `FsError { BadMagic, UnsupportedVersion, Truncated, EntryOutOfBounds }`.
- Pure helpers `read_u16_le`/`read_u32_le`/`entry_name` (NUL-trim), unit-tested in
  the self-test.

**Shell wiring** (`src/shell.rs`): extend `enum Command` with `Ls` and
`Cat(&str)`; `parse` adds `"ls" => Ls`, `"cat" => Cat(rest.trim())`; `dispatch`
adds `cmd_ls` (mount the boot image, print `name  size`) and `cmd_cat` (mount,
`read(name)`, print bytes as UTF-8 or `no such file: {name}`); `help` gains both;
`self_test` gains parse assertions for `ls`/`cat`.

**Ownership of the image.** The RAM disk is a `Vec<u8>` that must outlive every
`Fs` borrowing it. Simplest: a process-lifetime `static` built once. Since
`Vec::new` isn't const and we build it at runtime, hold it in a
`spin::Mutex<Option<Vec<u8>>>` (like `SCHED`) or a `OnceCell`-style init — or, for
the shell's `ls`/`cat`, simply call `boot_image()` and `mount` per command (the
image is tiny, ~100 bytes, and rebuilding it each `ls` is honest and stateless).
**Lean: rebuild per command** for M11 (no shared mutable state, no lifetime
gymnastics); revisit a cached mount if M12 needs one.

## 2. Preconditions

**On entry** (`fs::init`/`fs::self_test` called from `kernel_main`, before the
shell spawns):
- Long mode; heap up (the image is a `Vec<u8>`); paging on. Interrupts are already
  enabled by this point (`sti` ran before M10's shell), but the FS touches no
  interrupt state, no lock reachable from an IRQ, and no shared mutable
  statics — so IF state is irrelevant to it (unlike M9/M10). It's ordinary
  heap-allocating code.
- Must run **after** `heap::init` and **before** `shell::self_test`/the shell
  spawn, so `ls`/`cat` have a filesystem to read.

**On exit:** no persistent state required for M11 (each `ls`/`cat` mounts fresh).
The self-test proves the format round-trips and that malformed images are refused.

**The one real contract — `mount` is the whole trust boundary.** Every bound is
checked in `mount`; `list`/`read` assume a validated `Fs` and do only safe slicing.
A caller must not construct `Fs` except through `mount`.

## 3. Data flow

```
boot_image()  (writer / mkfs)
  └─ build a Vec<u8>: write superblock LE bytes, then N dir entries, then data
       returns raw bytes — NO struct shared with the reader

Fs::mount(&image)  (the trust boundary)
  ├─ len >= 16?                         else Err(Truncated)
  ├─ image[0..4] == "ZRFS"?             else Err(BadMagic)
  ├─ version == 1?                      else Err(UnsupportedVersion)
  ├─ 16 + 32*file_count <= len?  (checked_mul/add)  else Err(Truncated)
  ├─ for each entry: offset.checked_add(length) = Some(end) && end <= len
  │                                     else Err(EntryOutOfBounds)
  └─ Ok(Fs { image, file_count })

shell `ls`   -> Fs::mount -> list()  -> for e in entries: println!("{}  {}", e.name, e.length)
shell `cat X`-> Fs::mount -> read(X) -> Some(bytes) -> print as UTF-8
                                     -> None        -> println!("no such file: {X}")

fs::self_test (serial): mount good image -> assert names/sizes/exact motd bytes;
                        mount 4 corrupt images -> assert each specific FsError;
                        serial "M11: filesystem online"
```

The teaching boundary: **`&[u8]` in, named files out**, with the reader re-deriving
everything from bytes — never from a struct the writer handed it.

## 4. Edge cases

- **Truncated image** — too short for the 16-byte superblock, or the header's
  `file_count` claims a dir table longer than the image. Both → `Truncated`, via
  `checked_mul`/`checked_add` (a large `file_count` must not overflow the size
  computation and wrap to a small value — the classic integer-overflow parse bug).
- **Entry extent escapes the image** — `offset + length > len`, or `offset+length`
  *overflows* `usize`/`u32`. `checked_add` → `EntryOutOfBounds`. This is the
  headline security-relevant check (and the M16 seed): without it, `read` would
  slice out of bounds and panic.
- **Bad magic / wrong version** — `BadMagic` / `UnsupportedVersion`. Checked before
  anything is trusted.
- **Empty file (`length == 0`)** — valid; `read` returns an empty slice, `cat`
  prints nothing. Not an error.
- **`file_count == 0`** — valid empty FS; `ls` prints nothing (or "no files").
- **Name lookup** — exact match only; NUL-padding trimmed before compare. A name
  with no NUL terminator (fills all 20 bytes) is valid — trim must handle "no NUL
  found" (take all 20). A `cat` of a missing name → `None` → clean message, not a
  panic.
- **Non-UTF-8 file bytes in `cat`** — `motd.txt` is ASCII, but `cat` must not
  panic on arbitrary bytes: use `String::from_utf8_lossy` (or print byte-by-byte),
  never `from_utf8().unwrap()`.
- **Duplicate names in the directory** — `read` returns the first match; documented,
  not an error (the writer won't produce them, but the reader shouldn't assume).
- **Name/offset math off-by-one** — data region starts at exactly
  `16 + 32*file_count`; the self-test's exact-byte assertion catches a wrong base.

## 5. Verification plan (working vs. accidentally working)

The failure mode is a *silent parsing bug* returning plausible-but-wrong bytes, so
the tests assert exact bytes and exact errors — deterministic, on serial, no
keyboard needed.

1. **Round-trip, exact bytes.** `mount(boot_image())` → `list().len() == expected`;
   `list` contains `motd.txt` with `length == 13`; `read("motd.txt") == b"Hello,
   Ziran!"` **byte-for-byte**. The reader parses the writer's `&[u8]` with no
   shared struct — so this proves it decoded the layout, not that it read back its
   own object.
2. **Phantom file.** `read("nope") == None`.
3. **The four corrupt-input rejections (the real gate + M16 seed):**
   - flip a magic byte → `Err(BadMagic)`;
   - set version = 99 → `Err(UnsupportedVersion)`;
   - `mount(&image[..8])` → `Err(Truncated)`;
   - overwrite entry 0's offset with `0xFFFF_FFFF` → `Err(EntryOutOfBounds)`.
   A reader that returns the right bytes *and* refuses four specific malformations
   is provably parsing, not coincidentally succeeding.
4. **Overflow-specific case** — an entry with `offset` valid but `offset+length`
   overflowing → `EntryOutOfBounds` (proves the checked-arithmetic path, not just
   the `> len` compare).
5. **Interactive (manual `make run`):** `ls` lists the files; `cat motd.txt`
   prints `Hello, Ziran!`; `cat nope` reports cleanly. CI can't type, so it
   asserts the serial self-test + the `M11: filesystem online` marker; the shell
   parse arms are pinned by `shell::self_test` assertions.
6. CI marker `M11: filesystem online`; the Makefile `MILESTONE_MARKER` updated
   from the M10 marker to this one.

No GDB expected (pure safe Rust; a bug panics loudly or fails an assert). If a
value looks wrong, hex-dump `boot_image()` to serial and read it against the §1
layout table.

## 6. Build order (each its own small commit, QEMU-tested)

1. **Format + `mount`/validation.** `src/fs.rs`: constants, `read_u16/u32_le`,
   `FsError`, `Fs::mount` (full validation), a minimal `boot_image()`.
   `fs::self_test` covering mount + the four corrupt-input rejections. Wire
   `fs::self_test()` into `kernel_main` (after `heap`, before the shell); emit
   `M11: filesystem online`; bump the Makefile marker. CI green on the marker +
   rejections.
2. **Read path.** `list()` and `read(name)`; extend `self_test` with exact
   names/sizes and the exact `motd.txt` bytes; flesh out `boot_image()` with 2–3
   real files.
3. **Shell `ls`/`cat`.** Extend `Command`/`parse`/`dispatch`/`help`; add parse
   assertions to `shell::self_test`. Demoable interactively under `make run`.
4. **Docs + web.** `docs/concepts/filesystem.md` (the byte-by-byte hex-dump
   walkthrough — the payoff), the blog post, the `web/` step (predict "what will
   `cat motd.txt` print?" → run → explain the offset/length lookup; a natural
   "break it": corrupt the magic and watch `mount` refuse), STATUS/README. Then
   `/kernel-review` → `/retro` → `/document-milestone`. Skip `/red-team` with a
   one-line note (its real turn is M16; §5's corrupt cases are the seed).

## 7. Open unknowns (honest)

- **Per-command mount vs a cached static.** Leaning rebuild-per-command (stateless,
  no lifetime gymnastics, ~100-byte image). If `make run` shows it feels wasteful
  or M12 wants a persistent handle, switch to a `spin::Mutex<Option<Vec<u8>>>`
  built once at `fs::init`. Will decide at build time.
- **`ls`/`cat` output routing.** VGA (`println!`) for the human; the self-test
  asserts on serial. Confirm `cat` of the 自然 UTF-8 bytes renders on the VGA
  code page acceptably (it may show as two bytes — cosmetic; note it honestly).
- **`boot_image()` construction style.** Building the `Vec<u8>` with explicit
  `extend_from_slice` + LE writes is the readable choice; confirm the offsets line
  up with a first-run hex dump before trusting the self-test.
- **Whether `total_size`/`reserved` earn their bytes.** Keeping them for the
  teaching contrast with FAT's BPB and for a cross-check; if they feel like dead
  weight at build time, they stay documented as "deliberately redundant."
