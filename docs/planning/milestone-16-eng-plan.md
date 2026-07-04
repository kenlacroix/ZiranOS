# Milestone 16 — Break the filesystem boundary (flag capture): eng-plan

*Think phase done: `milestone-office-hours` (six forcing questions, below) +
`scope-guard` (**HOLD**; **Reduce** to the `data_end` ceiling only — no
floor/overlap/coverage accounting, no Tier-2 CTF bridge).*

## The one-sentence learning goal (office-hours Q1)

A bounds check is only as trustworthy as the bound it compares against. M11's
`mount` confirms a file's bytes are *in the image* (`offset + length <=
total_size`) but never that they *belong to that file* — so "in bounds" against
the wrong bound (the whole image) is not a real bound at all, and confining an
extent to the legitimate data region is the only thing between a read-only parser
and an arbitrary-disclosure primitive. This is **M15's confused deputy one layer
down** — offsets instead of pointers, the FS parser instead of the syscall.

## Done, as observable serial behavior (Q2/Q4)

One boot, four asserted outcomes, then a `M16:` marker CI greps:

1. **Navigation HELD.** `read_path("/FLAG")` and any guessed name → `NotFound`.
   No directory entry names the secret; resolution fails before any slice. (The
   boundary M12 already ships — kept as the contrast.)
2. **Loose extent LEAKS → flag captured.** A crafted image whose FILE entry's
   extent aliases the secret, run through `Fs::mount_loose` (the M11-era
   `total_size` bound), mounts; `read_path` returns the aliased bytes. The
   self-test asserts them **byte-equal** to `b"FLAG{ziran-boundary-leak}"` — a
   real capture, not "some bytes came back."
3. **Strict extent CONTAINS.** The *same* crafted image through `Fs::mount`
   (bounds file extents by `data_end`) → `Err(FsError::ExtentEscapesData)`, for
   the right reason (`offset + length > data_end`). Flag safe.
4. **Strict still PERMITS.** The legitimate boot image mounts under strict
   `Fs::mount` and every file reads **byte-exact** — the anti-"reject everything"
   guard (a check that denied all extents would pass test 3 while being useless).

## Minimal cut (Q3)

The loose-vs-strict pair on one crafted image is irreducible — that's the lesson.
Minimal cut: **one** new superblock field (`data_end`, reusing the reserved u32),
**one** new `FsError` variant (`ExtentEscapesData`; plus `BadDataEnd` for a
malformed field), and the strict/loose mount split. No writes, no generated fuzz
flood, **no** floor/no-alias/coverage accounting — just the `data_end` ceiling.
The floor (a FILE extent may still alias the superblock or a dir table) and a
fuzzing harness are named as red-team/future-post territory, not built here.

## Approach

- **Format v2 → v3 (one field).** Repurpose the superblock's reserved `u32` at
  `0x0C` (today written 0, never read — `fs.rs:377`) as **`data_end`**: the
  absolute offset where addressable file data ends. `boot_image()` bumps `VERSION`
  to 3 and writes `data_end = data_off + data_total` — i.e. the start of the
  secret. The secret then lives in `[data_end, total_size)`: present, past the
  addressable region. This mirrors M12's "the reserved byte became the `kind`
  byte" — a format that grows one meaningful field per milestone.
- **The gap, and the fix.** Today `validate_dir` (`fs.rs:285-292`) checks only
  `c_off + c_len <= image.len()` and the `KIND_FILE` arm is empty. The fix adds,
  for `KIND_FILE` only, a confinement to the declared data region:
  `c_off.checked_add(c_len) <= data_end` → else `ExtentEscapesData`. Directory
  *tables* keep their existing high-water-mark disjointness + in-image bound
  (they're metadata below `data_off`, and a dir pointing at the 25-byte secret
  fails `BadDirLength` anyway — `25 % 32 != 0`).
- **The teaching twin (the "decision you can feel").** Factor the parse into
  `mount_inner(image, confine: bool)`. `Fs::mount` = `mount_inner(.., true)` (the
  real, strict reader). `Fs::mount_loose` = `mount_inner(.., false)` — the M11-era
  behavior, bounding FILE extents by `image.len()` instead of `data_end`. Same
  parser, one bound differs; kept solely so the self-test can drive the identical
  crafted image through both. Labeled, like M15's `SYS_WRITE_UNCHECKED`, as a
  teaching device that must not exist in a hardened parser.
- **`data_end` sanity.** `mount_inner` validates `SUPERBLOCK_LEN <= data_end <=
  total_size` → else `BadDataEnd`. (This bounds a *malformed* field; it does **not**
  make `data_end` trustworthy against a fully-forged superblock — see the caveat.)

## Preconditions (machine state)

- Pure Rust over a `&[u8]`; runs at boot in `fs::self_test()` after the heap is up
  (`boot_image()` builds a `Vec<u8>`). No paging/interrupt/CPL preconditions — the
  whole point of ZranFS is that it's a total, panic-free function of its input
  bytes (the M12 contract: *never panic, never hang; only `Ok` or a specific
  `Err`*). M16 must preserve that on every new path, including the crafted images.
- No `unsafe`, no MMIO. `read_path` returns a zero-copy `&[u8]` borrowing the
  image; lifetimes unchanged.

## Data flow

```
boot_image() -> Vec<u8>  [v3: superblock.data_end = data_off+data_total; secret at tail]

attack (self-test):
  craft = boot_image(); read data_end from craft[0x0C];
  overwrite root entry 0 -> name "leak", offset=data_end, length=SECRET.len(), kind=FILE
    (its extent now aliases [data_end, total_size) == the secret)

  Fs::mount_loose(&craft)  -> validate_dir(confine=false): c_end <= image.len() -> Ok
     read_path("/leak")    -> &craft[data_end .. data_end+25]  == SECRET   [CAPTURED]

  Fs::mount(&craft)        -> validate_dir(confine=true):  c_end(=total) > data_end
                                                            -> Err(ExtentEscapesData)  [CONTAINED]

  Fs::mount(&boot_image()) -> Ok; every legit file reads byte-exact            [PERMITS]
```

## Edge cases (Q5 risks + the ones that bite parsers)

- **Off-by-one at the ceiling.** `data_end` is exclusive of the secret but a legit
  file may end *exactly* at `data_end`. `c_end == data_end` must pass; `c_end ==
  data_end + 1` must fail. Explicit boundary assertions for both.
- **Overflow.** `c_off + c_len` uses `checked_add` (already there) → `EntryOutOfBounds`
  on wrap, before the `data_end` compare.
- **Malformed `data_end`.** `> total_size` or `< SUPERBLOCK_LEN` → `BadDataEnd`.
- **A `KIND_DIR` entry aliasing the secret.** 25 bytes ⇒ `25 % 32 != 0` ⇒
  `BadDirLength`; even if it parsed, `cat` on a dir is `IsADirectory`. Not a clean
  exfil path — the FILE extent is the target; noted, not specially handled.
- **The format bump breaking the existing 10 corrupt-image tests.** They mutate
  `boot_image()` output (now v3). Rebuild + boot **after** the writer/reader change
  and **before** adding the attack, so a format regression surfaces alone. The
  "bad version = 99" test still trips `UnsupportedVersion`; verify the version
  offset/const updates are consistent.
- **Zero-length file** (`c_len == 0`): `c_end == c_off`; passes iff `c_off <=
  data_end`. Fine.

## Verification plan (proving, not observing)

- **Serial asserts** for all four outcomes. Capture asserts byte-equality with the
  planted `SECRET`. Containment asserts the specific `ExtentEscapesData`, not a
  generic error. The PERMITS leg reads real files byte-exact under strict `mount`.
- **Boundary assertions**: an entry with `c_end == data_end` mounts strict-OK; the
  same +1 → `ExtentEscapesData`. Pinned so the off-by-one can't regress.
- **Preserve the corpus**: all pre-existing corrupt-image rejections still pass
  under v3 (now 12+ cases). The `self_test` serial line + the `M16:` marker are the
  CI gate; bump `Makefile` `MILESTONE_MARKER` to `M16: filesystem boundary`.
- **The M12 totality contract holds**: every new crafted image yields `Ok` or a
  *named* `Err` — never a panic, never a hang. (The crafted aliasing image is the
  proof: it must mount-and-leak or reject cleanly, not slice out of bounds.)

## The honest caveat (for the blog / concept doc)

`data_end` lives *in the image*, so an attacker who can rewrite the **superblock**
(not just a directory entry) can forge `data_end = total_size` and reopen the
leak. M16 confines the **entry** attack surface — the documented gap, the classic
"corrupt a directory entry" scenario — and names the residual explicitly. This is
the same shape as M15's deepest lesson (validating against a value the caller
controls is not validation) and ties to the project's core honesty: this OS will
never withstand a real adversary. Fully closing it needs either structural
coverage accounting or an out-of-band authority — out of scope, and stated as
such.

## Red-team note

M16 *is* a red-team milestone. Per the "write the attack first" rule, the crafted
aliasing image is the acceptance test the fix is built against, aimed at the seam
the M11 eng-plan and `fs.rs:155-159` both flag. A full `/red-team` pass (fuzz
offsets/lengths/overflow at the extent boundary, the metadata-alias floor, the
forged-`data_end` residual) runs after the build lands; findings become pinned
self-test assertions, as in M15.
