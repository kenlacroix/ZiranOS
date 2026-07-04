# Milestone 12 — file manager (ZranFS v2 subdirectories) — eng-plan

Locks the technical approach before building. The **end goal** (PLAN §5): "list,
navigate, and read files." Grounded in a two-way research fan-out (the ZranFS v2
subdirectory format + termination; the shell navigation + path math). Companion
concept doc: `docs/concepts/filesystem.md` gets an M12 addition during the build.

The milestone teaches one thing: **a directory is the same lie as a file, told
recursively** — a directory is a file whose bytes are more `{name, offset,
length}` entries. The hard part is not reading the tree but *validating* it: a
`mount` that recurses through an attacker-controlled tree and **provably cannot
loop**.

## 0. Scope-guard (do this first)

Verdict **Hold**. The capstone is where "file manager" tempts a real filesystem.
The deferrals:

- **Read-only, depth-2, tree-shaped.** Defer write/`mkdir` (free-space
  management, M11 already deferred it), symlinks/hard links (a DAG the
  forward-ordering rule deliberately doesn't assume), permissions/timestamps/
  `stat` (no metadata beyond name/size/kind — the multi-user non-goal), a path
  cache / open-file table (rebuild-per-command stays), and arbitrary-depth trees
  (the format admits any depth the rule allows; ship+test depth 2 only, so no
  depth-bomb surface — that's M16's).
- **The `kind` byte is the entire format delta** — reuse the u32 M11 kept
  "reserved for the FAT-BPB teaching contrast." No new fields, no superblock
  change beyond the version bump.
- **The hidden secret is a teaching hook, not creep** — unreferenced bytes in the
  `mkfs` image that make the *already-planned* M16 exfil target concrete (PLAN
  §8). Zero runtime cost.

Reaching M12 **completes the core arc** (boot → memory → interrupts → tasks →
shell → files) — the project's primary success criterion is met here; M13–16 are
stretch/security. Record that in PLAN/STATUS.

## 1. Approach

Extend `src/fs.rs` (v2 format + recursive validation + a path resolver) and
`src/shell.rs` (cwd + `canonicalize` + `cd`/`pwd`/`ls`/`cat`). No asm, no
`unsafe`, no new hardware — pure safe Rust over the heap `Vec<u8>`, like M11.

**Format — ZranFS v2** (everything from v1 unchanged except two things):

```
Superblock (16 bytes) — byte-identical to v1, only `version` now reads 2.

Directory entry (32 bytes) — the ONLY delta is the last field:
  +0x00 20  name     ASCII, NUL-padded            [unchanged]
  +0x14  4  offset   u32 LE: byte offset to node   [unchanged]
  +0x18  4  length   u32 LE: region length         [unchanged]
  +0x1C  4  kind     u32 LE: 0=FILE, 1=DIR          [was `reserved`]

A FILE entry's [offset..offset+length) is opaque file bytes (v1 behaviour).
A DIR  entry's [offset..offset+length) is an ARRAY of `length/32` child entries
       — the same 32-byte rows, recursively. length%32 must be 0; length==0 is a
       valid empty directory. The root table (at 16, `file_count` entries) IS the
       top-level directory.
```

New constants: `VERSION: u16 = 2`, `ENT_KIND: usize = 0x1C`, `KIND_FILE: u32 = 0`,
`KIND_DIR: u32 = 1`.

**The termination rule (the heart of the milestone).** A malformed image can make
a `DIR` `offset` point back at itself, an ancestor's table, or the superblock — a
cycle a naive recursive validator loops on forever. The rule:

> **A child directory's region must begin at an offset `>=` the end of the parent
> directory's region.** (`c_off >= p_off + p_len`.)

Termination proof: along any root-to-descendant descent, region start offsets are
**strictly increasing** (a non-empty parent has `p_end > p_off`, and the rule
forces `c_off >= p_end > p_off`). A strictly increasing sequence of non-negative
integers all `< image.len()` has length `<= image.len()`. So recursion depth is
bounded by `image.len()`; the walk cannot fail to terminate. Cycles/back-edges
require `c_off < p_end`, which the rule rejects as `DirNotForward`. **No depth cap
needed** — the geometry forbids the loop; a depth cap would conflate a deep-but-
valid tree with a cycle. Bonus: `c_off >= p_end` also means parent and child
*tables* can never overlap (a partial answer to M11's "extents may alias").

```rust
// Called by mount after the v1 superblock/magic/version/size/dir-table-fits
// checks (which establish the root table bytes are in-bounds).
fn validate_dir(image: &[u8], off: usize, len: usize, forward_min: usize) -> Result<(), FsError> {
    if off < forward_min          { return Err(FsError::DirNotForward); }   // cycle / back-edge / superblock
    if len % DIRENT_LEN != 0      { return Err(FsError::BadDirLength); }
    let end = off.checked_add(len).ok_or(FsError::EntryOutOfBounds)?;
    if end > image.len()          { return Err(FsError::EntryOutOfBounds); }
    for i in 0..(len / DIRENT_LEN) {
        let base = off + i * DIRENT_LEN;
        let c_off = read_u32_le(image, base + ENT_OFFSET) as usize;
        let c_len = read_u32_le(image, base + ENT_LENGTH) as usize;
        let c_end = c_off.checked_add(c_len).ok_or(FsError::EntryOutOfBounds)?;
        if c_end > image.len()    { return Err(FsError::EntryOutOfBounds); }
        match read_u32_le(image, base + ENT_KIND) {
            KIND_FILE => {}                                    // in-bounds is sufficient
            KIND_DIR  => validate_dir(image, c_off, c_len, end)?,  // child must be >= this region's end
            _         => return Err(FsError::BadKind),
        }
    }
    Ok(())
}
// mount seeds: validate_dir(image, SUPERBLOCK_LEN, file_count*DIRENT_LEN, SUPERBLOCK_LEN)
```

**Path resolution — split cleanly between shell and fs.**
- **The shell owns `canonicalize(cwd, arg) -> String`** (pure string: join, drop
  `.`/empty, `..` pops, never past root; result canonical absolute). This is UX
  logic, lives with the prompt, and is unit-tested in `shell::self_test`.
- **The fs `resolve(abs: &str) -> Result<Node, FsError>`** takes an *already-
  canonical absolute path* and walks `DIR` entries from the root — no `.`/`..`,
  no `cwd`, no parent pointers (it only ever descends). `Node = File{off,len} |
  Dir{off,len}`.

fs surface for the shell: `resolve(abs)`, `list_dir(abs) -> Result<Vec<DirEntry>,
FsError>` (resolves to a Dir, lists its region; `DirEntry` gains `is_dir: bool`),
`read_path(abs) -> Result<&[u8], FsError>` (resolves to a File). Resolution-time
errors: `NotFound`, `NotADirectory`, `IsADirectory`.

**The `mkfs` tree** (`boot_image`): a depth-2 tree, well under a few KB.
```
/  (root, 4 entries)
├─ motd.txt   FILE  "Hello, Ziran!"
├─ readme     FILE  "Ziran OS -- a from-scratch x86_64 kernel.\n"
├─ ziran.txt  FILE  the 自然 line (unchanged)
└─ docs/      DIR  -> 2-entry child table
     ├─ filesystem.txt  FILE  "A file is a lie a header tells about bytes.\n"
     └─ shell.txt       FILE  "cd, pwd, ls, cat -- navigation over ZranFS v2.\n"
[hidden]  b"FLAG{ziran-boundary-leak}"  — bytes in the image, NO entry names them
```
Physical layout (satisfies forward-ordering automatically): superblock, root
table `[16..16+128)`, docs child table `[16+128..16+192)` (starts at root_end →
forward-OK), then file data, then the hidden secret last. `total_size =
img.len()` (covers the secret; `mount` requires no full byte coverage, so
unreferenced bytes are legal and invisible).

**Shell wiring:** `Command` gains `Pwd`, `Cd(&str)`, and `Ls`/`Cat` take a path
arg; `dispatch(cwd: &mut String, line)`; `shell_main` owns `let mut cwd =
String::from("/")` and a `print_prompt(&cwd)` → `ziran:/docs> `. `cd` canonicalizes
then confirms `is_dir` before mutating cwd; `ls`/`cat` canonicalize `(cwd, arg)`
and call `list_dir`/`read_path`. Per-command `mount` stays (image ~250 bytes).

## 2. Preconditions

**On entry** (`fs::self_test` from `kernel_main`, before the shell; `cd`/`ls`/`cat`
from the shell task): long mode, heap up (the image + `canonicalize`/`resolve`
`Vec`/`String` allocate), interrupts on. The fs touches no interrupt state, no
lock reachable from an IRQ, no shared mutable statics — IF state is irrelevant to
it (pure heap-allocating code), exactly as M11.

**The mount contract, strengthened:** *never panic and never loop on any input —
only `Ok` or a specific `Err`.* v1 gave the no-panic half (checked arithmetic,
in-bounds slicing); v2 adds the no-loop half via the forward-ordering invariant.
A caller must construct `Fs` only through `mount`; `resolve`/`list_dir`/`read_path`
assume a validated tree and do only safe slicing + descent.

**Shell cwd invariant:** `cwd` is always canonical (leading `/`, no `.`/`..`, no
trailing slash except root `"/"`). `canonicalize` preserves this; every command
trusts it.

## 3. Data flow

```
boot_image()  (writer/mkfs)
  └─ lay down: superblock(v2) ; root table ; docs child table ; file data ; SECRET
       root's `docs` entry: kind=DIR, offset=docs table, length=2*32
       returns raw Vec<u8> — no struct shared with the reader

Fs::mount(&image)  (trust boundary, now recursive)
  ├─ v1 checks: len>=16, magic, version==2, total_size==len, root table fits
  └─ validate_dir(root)  ->  recurses DIR children, forward-ordering + in-bounds
                              -> Ok | Err(BadDirLength|DirNotForward|BadKind|EntryOutOfBounds|...)

shell `cd docs`  (cwd="/") -> canonicalize("/","docs")="/docs"
                            -> fs.resolve("/docs") == Dir  -> cwd="/docs"
shell `ls`       (cwd="/docs") -> fs.list_dir("/docs") -> [filesystem.txt, shell.txt]
shell `cat filesystem.txt` (cwd="/docs") -> canonicalize -> "/docs/filesystem.txt"
                            -> fs.read_path(...) -> Some(bytes) -> print
shell `cd ..`    -> canonicalize("/docs","..")="/" -> cwd="/"

fs::self_test (serial): mount tree ; resolve/list/read byte-exact through docs/ ;
                        reject cycle(DirNotForward)/bad-len(BadDirLength)/bad-kind ;
                        assert the secret is unreachable ; "M12: file manager online"
shell::self_test: canonicalize(...) exhaustive ; parse arms for cd/pwd/ls/cat
```

The handoff boundary: **shell canonicalizes (string) → fs resolves (walk) → slice**.
`..` is a `Vec::pop` in the shell, never a disk operation.

## 4. Edge cases

- **Directory cycle** (`DIR` offset ≤ an ancestor's start) → `DirNotForward`, by
  the forward rule. *Must not hang* — the self-test asserts the error.
- **Self-referential dir** (offset points at its own region) → `off < forward_min`
  → `DirNotForward`.
- **Dir length not a multiple of 32** → `BadDirLength` (can't misread a partial
  trailing entry).
- **Unknown kind** (`> 1`) → `BadKind` (future format bits must be explicit, never
  silently a file).
- **Overflow** (`offset+length` wraps, or dir region wraps) → `checked_add` →
  `EntryOutOfBounds` (now on dir regions too).
- **Empty directory** (`length == 0`) → valid; `ls` prints `(empty)`.
- **`file_count == 0`** → valid empty root.
- **`canonicalize`:** pop-past-root is a no-op (`/`+`..`→`/`); trailing slash
  dropped (`docs/`→`/docs`); `//x//`→`/x`; empty arg → stay; absolute arg ignores
  cwd. All in the self-test table.
- **`cd` onto a file** → `NotADirectory`, cwd unchanged (a bad `cd` never moves
  you). **`cat` onto a directory** → `IsADirectory` (re-classify the `None` for a
  precise error). **`cd`/`cat` missing** → `NotFound`.
- **The hidden secret:** no path resolves to it (no entry names it), so `resolve`
  returns `NotFound` for any guess — the boundary holds. (M16's job: make a
  crafted image leak it.)
- **DAG** (two entries naming the same child offset) → still terminates
  (offsets strictly increase per descent), merely re-validates a region;
  harmless for M12, dedupe is an M16 nicety.
- **Non-UTF-8 file bytes in `cat`** → `from_utf8_lossy`, never panic (as M11).

## 5. Verification plan (working vs. accidentally working)

The new failure mode is a *hang* (cycle) and a *silent wrong-slice* (bad walk).
Both are caught deterministically on serial, no keyboard:

1. **Tree round-trip, byte-exact.** `mount(boot_image())`; `resolve("/docs")` is a
   `Dir`; `list_dir("/docs")` has 2 entries incl. `filesystem.txt`;
   `read_path("/docs/filesystem.txt")` **equals its literal bytes**. Proves the
   resolver walked into a subdirectory and decoded it, not guessed.
2. **`..`/`.` canonicalization** (in `shell::self_test`, the pure function): the
   full edge table (`/docs`+`..`→`/`, `/a/b`+`../c`→`/a/c`, pop-past-root,
   `//x//`→`/x`, absolute ignores cwd, …).
3. **The termination gate (the headline):** a hand-built image with a `DIR` whose
   offset points back at the root → `mount` returns `Err(DirNotForward)` **and the
   test completes** (a hang would time out the 20 s boot → FAIL, which is itself
   the signal). Plus `BadDirLength` (dir length `% 32 != 0`) and `BadKind` (kind
   = 99). These extend M11's five rejections.
4. **The boundary holds:** assert `resolve` / `read_path` for the secret's *name*
   is `NotFound`, and that no `list_dir` anywhere surfaces it — the planted flag
   is present in the image yet unreachable. (The office-hours honesty check + the
   M16 setup.)
5. **Interactive (manual `make run`):** `cd docs` → prompt `ziran:/docs>`, `ls`,
   `cat filesystem.txt`, `cd ..`, `cat docs` → `is a directory`, `cd nope` → error.
   CI asserts the serial self-test + `M12: file manager online`.
6. CI marker `M12: file manager online`; Makefile `MILESTONE_MARKER` bumped from
   M11.

**GDB plan (Q5):** if `mount` ever hangs, `make debug` + break in `validate_dir`,
inspect the descent offsets (they must strictly increase). But the forward rule
should make this unreachable; the self-test's cycle case is the guard.

## 6. Build order (each its own small commit, QEMU-tested)

1. **v2 format + recursive validation** (no tree yet). Add `VERSION=2`, `ENT_KIND`,
   `KIND_FILE/DIR`, the new `FsError` variants (`BadDirLength`, `DirNotForward`,
   `BadKind`, and resolution-time `NotFound`/`NotADirectory`/`IsADirectory`), and
   `validate_dir`. Keep `boot_image` flat (all `kind=0`) so the M11 round-trip
   still passes. Extend `fs::self_test` with the three new mount rejections
   (cycle, bad-len, bad-kind). CI stays on the M11 marker this step.
2. **Boot tree + resolver.** Grow `boot_image` to the depth-2 tree + hidden secret;
   add `Node`, `resolve`, `list_dir`, `read_path`, `DirEntry.is_dir`. Extend
   `fs::self_test`: navigate into `docs/`, byte-exact read, `IsADirectory`/
   `NotFound`, and the secret-unreachable assertion.
3. **Shell `cd`/`pwd`/`ls`/`cat` + cwd.** `canonicalize` (pure), the `Command`
   arms, cwd threading, `print_prompt`, help; `shell::self_test` canonicalize +
   parse assertions (fix the existing `Ls` assertion for its new shape). Emit
   `M12: file manager online`; bump the Makefile marker. Interactive demo under
   `make run`.
4. **Docs + web.** Concept-doc addition ("the directory is the same lie,
   recursively" + the termination proof + the hidden-secret/M16 tie-in); blog
   post; `web/` step (navigate into `docs/`, predict `cat`, then "can you reach the
   secret? — no, here's why"); STATUS/README (note the core arc is complete).
   Then `/kernel-review` (focus: the termination argument) → `/retro` →
   `/document-milestone`. `/red-team`: a real FS boundary now exists — run it, or
   defer to M16 with a one-line note (the exfil target is now planted).

## 7. Open unknowns (honest)

- **Where `canonicalize` lives.** Plan: the *shell* owns it (pure, tested in
  `shell::self_test`), and `fs::resolve` takes the already-canonical absolute path.
  The fs brief proposed fs owning it; the shell brief proposed the shell. Shell-
  owns is cleaner (no cwd in the fs, resolver only descends) — going with that;
  will confirm no duplication at build.
- **`DirEntry.is_dir` vs a `kind` field.** Leaning a bool derived from the kind
  byte (simplest for `ls`); confirm the M11 self-test's `DirEntry` assertions
  still hold with the added field.
- **`resolve` return borrow lifetime** — `read_path` returns `&'a [u8]` into the
  image; same local-copy pattern as M11's `read` (`let image = self.image;`).
  Confirm the walk doesn't tangle the borrow checker.
- **Keeping M11's `read(name)`/`list()`** — become `read_path`/`list_dir` at root;
  decide whether to keep thin wrappers or update the M11 self-test/shell call
  sites. Leaning: replace, update call sites, keep the module lean.
- **Interactive prompt width** — `ziran:/docs> ` is fine; confirm a deep cwd
  doesn't wrap awkwardly on the 80-col VGA (depth 2 is short).
