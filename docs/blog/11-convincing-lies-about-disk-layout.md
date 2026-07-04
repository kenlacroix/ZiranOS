# Files are just convincing lies about disk layout

*Milestone 11 — filesystem (read). The kernel gets something to `ls` and `cat`: a
read-only filesystem that turns a flat run of bytes into named files — and the
lesson is what a "file" actually is.*

> New to this? Read
> [docs/concepts/filesystem.md](../concepts/filesystem.md)
> alongside — it explains how a directory table of `{name, offset, length}` is the
> *entire* difference between a byte array and named files you can list and read,
> and why the interesting, security-relevant code isn't the reading but the
> *refusing*: the mount-time validation that turns a malformed image into a clean
> error instead of a wrong answer.

Last milestone gave the kernel a *shell*: a prompt that reads what you type and
runs a command. This one gives it something to `ls` and `cat`. The shell already
knew how to read live kernel state (`mem`, `ps`); now it reads *files*. But the
milestone isn't really about storage — it's about what a "file" is underneath the
name. A file is a lie a header tells about a flat run of bytes. A directory table
of `{name, offset, length}` is *all* that separates a byte array from named files.
M11 builds the round-trip and, more importantly, the trust boundary: a writer packs
little-endian bytes, and a reader re-derives the directory trusting *nothing* but
the array.

### Milestone 11 — filesystem (read) — checklist

Think
- [x] /milestone-office-hours — six forcing questions answered below
- [x] /scope-guard — verdict recorded: **Hold**, below (resolved PLAN §7's FAT16-vs-custom question toward custom)

Plan
- [x] /eng-plan — approach, byte layout, the trust boundary, edge cases, verification — [docs/planning/milestone-11-eng-plan.md](../planning/milestone-11-eng-plan.md)

Build
- [x] Working, demoable state reached (`ls` lists the RAM-disk files with sizes; `cat motd.txt` prints its bytes; `cat nope` reports "no such file")
- [x] `make` is clean: assembles, compiles, links; `make check-header` passes
- [x] No new compiler warnings; no new `unsafe` — the whole filesystem is pure safe Rust (a bad offset is a loud panic or a caught `Err`, never a silent success)

Debug (as needed)
- [x] /investigate — not needed; zero runtime debugging cycles (the one finding was caught in review, below)
- [x] The real debugging story captured — below (the honest version: nothing broke)

Review
- [x] /kernel-review — 2-lens pass (the mount trust boundary as a complete bound-checker; the shell's ls/cat); one finding fixed

Security
- [x] /red-team **skipped** — no privilege boundary yet (still ring 0, one address space; the FS is a read-only in-RAM structure, not a syscall gate). Its real turn is M16, though this milestone's 5 corrupt-input rejections are the seed of that work.

Reflect
- [x] /retro — post-mortem answered honestly, below
- [x] /document-milestone (STATUS/README/this post)
- [ ] CI green (build + headless QEMU boot) — runs on push

## Office hours

1. **The one thing I'll understand.** That a "file" is a *lie* a header tells about
   a flat run of bytes. A directory table of `{name, offset, length}` is *all* that
   separates a byte array from named files you can `ls` and `cat`. The milestone is
   the round-trip made honest: a writer packs little-endian bytes into a buffer, and
   a reader re-derives the directory trusting nothing but that array — no shared
   struct, no back channel. If the read works, the reader *decoded the layout*.

2. **"Done" as observable behavior.** At the `ziran>` prompt, `ls` lists the
   RAM-disk files with their sizes (`motd.txt` 13 bytes, `readme`, `ziran.txt`);
   `cat motd.txt` prints `Hello, Ziran!`; `cat nope` reports `no such file`. On
   serial (CI): `fs::self_test` mounts the boot image, asserts `motd.txt`'s *exact*
   bytes, and rejects five deliberately-corrupt images — each with its own specific
   error — then prints `M11: filesystem online`.

3. **The smallest version that still teaches it.** Read-only, one flat directory,
   contiguous data, exact-name lookup. A 16-byte superblock (`ZRFS` magic, version,
   `file_count`, `total_size`), 32-byte directory entries (20-byte name + `u32`
   offset + `u32` length), then flat data. The "disk" is a heap `Vec<u8>` built
   in-kernel by a tiny `mkfs` — **no external image, no `include_bytes!` blob, no
   build magic.** Two or three files. No writing, no directories, no blocks, no
   caching, no VFS.

4. **Working vs. accidentally working.** The reader parses the raw `&[u8]` the
   writer produced with *no shared structs* — so a passing read means it decoded the
   layout, not that it read back its own object. The self-test asserts *exact bytes*
   (`cat motd.txt == b"Hello, Ziran!"`), not "non-empty." And the real gate is the
   corrupt-input rejections: bad magic, wrong version, size mismatch, truncated,
   entry-out-of-bounds. A reader that returns the right bytes *and* refuses five
   specific malformations is provably parsing, not guessing.

5. **What most likely stalls this for days.** *Not* a triple-fault — this is pure
   safe Rust, no `unsafe`. The realistic failure is a silent parsing bug: a wrong
   offset in the writer or the reader (little-endian byte math, data region at
   `16 + 32*file_count`) that returns plausible-but-wrong bytes. Mitigation: the
   byte-exact self-test catches it immediately on boot. And the one genuine hazard —
   an out-of-bounds slice — *panics* (safe Rust makes it loud), which is exactly what
   mount validation turns into a clean `Err` before any read runs.

6. **Drifting toward a non-goal?** Three temptations, all deferred. FAT16 (a real
   format, but its complexity is *accidental* here, and a valid image effectively
   needs `mkfs.fat` — a host-tool build dependency PLAN §4 forbids; deferred as a
   future post). virtio-blk / a real block device (the "real hardware" non-goal).
   Write / VFS / caching (that's a *real* filesystem, later). Ran `/scope-guard` →
   **Hold**, and resolved PLAN §7's open FAT16-vs-custom question toward custom,
   recorded in PLAN.md.

## Scope-guard

Verdict: **Hold**. The milestone teaches the lie with the *least accidental
complexity*: a superblock, a directory, a data region, read-only, flat. Choosing a
custom format over FAT16 is the scope-*disciplined* answer, not a shortcut — a valid
FAT16 image effectively requires `mkfs.fat`, which drags a host-tool dependency and
an opaque prebuilt blob into the build: exactly the "build magic that hides how
bytes become data" that PLAN §4 forbids. An in-kernel `mkfs`-into-a-`Vec` keeps the
whole round-trip in readable Rust, visible at the source level. FAT16 is *deferred*
as a valuable future post, not rejected. And the five corrupt-input rejections are
required substance, not gold-plating: they're what make the parse provable, and
they're the seed of M16's boundary work.

## Eng-plan (summary)

A writer and a reader that share only a byte array:

- **`boot_image()`** is the in-kernel `mkfs` — the *writer*. It packs the 16-byte
  superblock (`ZRFS` magic at offset `0x00`, version, `file_count`, `total_size` at
  `0x08`), then one 32-byte directory entry per file (20-byte zero-padded name, a
  `u32` little-endian offset, a `u32` little-endian length), then the flat data
  region starting at `16 + 32*file_count`. It returns a `Vec<u8>` — the "disk."
- **`Fs::mount()`** is the *reader* and the single trust boundary. It re-derives the
  directory from the raw bytes, validating everything with *checked* arithmetic:
  the magic, the version, that `total_size` equals the image length, that the whole
  directory fits, and that *every* file extent lands inside the image. A failure is
  a specific `FsError`, never a panic.
- **`list()` / `read()`** hand back zero-copy slices into the mounted array. Because
  mount already proved every extent is in bounds, these can't panic.
- **`self_test`** mounts the boot image, asserts `motd.txt`'s exact bytes, and
  rejects five corrupt images (bad magic, wrong version, size mismatch, truncated,
  entry-out-of-bounds), then prints `M11: filesystem online`.
- **The shell** grows `ls` and `cat` (`Command`/parse/dispatch/help), mounting the
  image per command.

The writer/reader separation is the point: with no shared struct, a passing read
*means* the layout decoded correctly. Full plan in
[docs/planning/milestone-11-eng-plan.md](../planning/milestone-11-eng-plan.md).

## What got built

Five commits on `claude/ziran-os-plan-qgobid`:

- **M11 plan** — office-hours + scope-guard + eng-plan; resolved the FAT16-vs-custom
  question in PLAN.md (toward custom).
- **M11 core** — `src/fs.rs`. `boot_image()`, the in-kernel `mkfs` (the writer);
  `Fs::mount`, the reader and trust boundary (validates magic / version /
  `total_size` / directory-fits / every-extent-in-bounds with checked arithmetic);
  `list()`/`read()` as zero-copy slices; **no `unsafe`**. `self_test` with the
  corrupt-input rejections and the marker `M11: filesystem online`.
- **M11 shell** — `ls`/`cat` in the shell (`Command`/parse/dispatch/help), mounting
  the image per command; parse assertions pinned.
- **M11 review** — a 2-lens `/kernel-review`. It confirmed that *no* malformed image
  escapes `mount` to panic in `list`/`read` (the trust boundary is a complete
  bound-checker) and that the shell has no parsing bugs. It fixed one real finding:
  `total_size` was **written-but-unchecked** — the comment claimed a cross-check the
  code didn't perform. `mount` now validates it (`FsError::SizeMismatch`, the *fifth*
  rejection). It also documented that extents aren't constrained to the data region
  (harmless — an M16 seed) and pinned `cat`'s argument handling.

## Verification status (honest)

Boot-tested live under headless QEMU. The serial log:

```
[ok] fs: mounted 3 files, motd.txt verified byte-for-byte, 5 corrupt images rejected
M11: filesystem online
[ok] shell: editing, parsing, input ring, and ps/mem accessors verified (tasks=3, heap_free=1015568)
[boot test] PASS -- reached long mode and 'M11: filesystem online'
```

What this actually proves — and what it doesn't:

- `fs::self_test` mounts the boot image and asserts `motd.txt`'s **exact bytes**.
  Because the reader shares no struct with the writer, this proves it *decoded the
  layout* — it didn't round-trip its own object. A wrong offset in either the writer
  or the reader would return the wrong bytes and fail the assert.
- It then rejects **five corrupt images**, each with its specific `FsError` (bad
  magic, wrong version, size mismatch, truncated, entry-out-of-bounds). This is the
  real gate: returning the right bytes *and* refusing five specific malformations is
  what makes the parse provable rather than coincidental.
- The shell self-test pins `ls`/`cat` **parsing** (verb, argument, the `no such
  file` path).
- **Interactive `ls`/`cat` output on VGA is a manual check.** The actual rendered
  listing and file contents in response to real commands are verified by hand under
  `make run` — CI can't watch a screen. But the read path itself and the five
  rejections are pinned on serial, so the manual piece is a thin shell around
  already-proven logic.

The full 20-second run had no panics and no faults.

## Retro

**Plan vs. reality.** Accurate end to end: the byte layout, `mount` as the *single*
trust boundary, per-command re-mount, the five-step validation, and the
corrupt-input gate all landed as planned — no revisions. The one under-specified
spot: `total_size` was listed as a "cross-check" field, but the plan never pinned
whether `mount` actually *validates* it. The first implementation wrote it at offset
`0x08` but never read it back — the comment promised a check the code lacked. The
adversarial review caught the gap.

**What broke, and for how long.** Nothing — zero debugging cycles, the **third**
milestone in a row (M9, M10, M11). Pure safe Rust removes the "silent corruption"
class entirely: a bad offset is a loud panic or a caught `Err`, never a subtly-wrong
success that hides for days.

**The root cause / the fix.** There was no bug. The review finding was a
*written-but-unchecked field*: `boot_image` laid down `total_size` and the comment
called it a cross-check, but `mount` never read offset `0x08`. Fix: `mount` now
asserts `total_size == image.len()` (`FsError::SizeMismatch`) — a real fifth
rejection. The comment now describes code that exists.

**The assumption worth re-examining (the lede).** *"A filesystem is complicated."*
It isn't. A read-only one is three structures and some little-endian byte math — and
the *reading* is the trivial part (just slicing an array you've already validated).
The substance is the opposite of what "filesystem" implies: not reading the bytes
but **refusing** to read a lie that doesn't check out. The genuinely hard,
security-relevant code lives entirely in `mount`'s validation — which is also
exactly why FAT16's complexity is *accidental* to the lesson. A "file" teaches that
the trust boundary is the whole game.

**What earned its keep.** The writer/reader separation with **no shared structs**.
`boot_image` produces a `Vec<u8>`; `Fs::mount` re-derives the directory from those
raw bytes, trusting only the array. That's what makes the byte-exact self-test
*mean* something — a round-trip through a shared struct would pass even if the layout
were wrong. Paired with the five corrupt-input rejections it forms a complete
working-vs-accidentally-working gate: the adversarial reviewer couldn't construct an
image that escapes `mount` *because* every bound is checked. It's a reusable pattern
for any parser, and the seed of M16.

**One thing to do differently.** Don't write a field or a comment that claims a
validation until the validation exists. The `total_size` "cross-check" was
*aspirational documentation* — the comment ran ahead of the code. Write the check
first, or mark the field explicitly "reserved / not yet validated."

## Takeaway for the next person

A "file" is a convincing lie: a flat run of bytes plus a header that names regions
of it. Building a read-only filesystem teaches that the reading is trivial — the
whole game is the *trust boundary*, the code that refuses a malformed image instead
of returning a wrong answer. Keep the writer and the reader sharing nothing but the
byte array, and a passing read *proves* you decoded the layout; add a handful of
deliberately-corrupt images the reader must reject, and you've drawn the exact line
between working and accidentally working. That validation is also where the real,
security-relevant work lives — which is why the next boundary milestone (M16) starts
here.

Next: **Milestone 12 — the file manager.** The kernel can now `ls` and `cat` a
read-only image; next it gets a program to browse it — the file manager that was the
stated end goal all along.
