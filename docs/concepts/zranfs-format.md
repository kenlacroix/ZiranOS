# The ZranFS on-disk format, byte by byte

*A build-it-yourself companion to [filesystem.md](filesystem.md) and the
[filesystem CTF](../../web/ctf.html). That doc explains why a filesystem is really
a **lie a header tells about a flat run of bytes**; this one hands you the exact
bytes so you can tell that lie yourself — write a ZranFS image from scratch, honest
or crafted, and feed it to the kernel's `load` command. It is also the on-ramp for
the [Tier-2 remote capture](../../web/ctf-remote.html): there the flag sits at a
per-session offset the kernel prints, past your upload, so you can't reuse a fixed
image — you have to build one. Everything below mirrors [`src/fs.rs`](../../src/fs.rs)
(the reader is the spec; if the two ever disagree, `fs.rs` wins). All integers are
**little-endian**.*

---

## The shape

An image is three regions, back to back:

```
[ 16-byte superblock ][ N × 32-byte directory entries ][ file bytes … ]
```

The superblock says how many entries the root directory has and where the "real"
data region ends. Each entry names a node and points at a `[offset, offset+length)`
window into the image. A file's window is its bytes; a directory's window is *more
entries* — the same lie, one level down. That recursion is the whole of
subdirectories; this primer stays at the root, which is all the CTF needs.

## The superblock (16 bytes, at offset 0)

| Offset | Size | Field | Meaning |
|-------:|-----:|-------|---------|
| `0x00` | 4 | `magic` | ASCII `ZRFS`. Checked first. |
| `0x04` | 2 | `version` | must be `3`. |
| `0x06` | 2 | `file_count` | number of root directory entries. |
| `0x08` | 4 | `total_size` | the declared image length. |
| `0x0C` | 4 | `data_end` | the ceiling of the "real" data region — every **file** extent must end `<= data_end`. |

## A directory entry (32 bytes each, starting at offset `0x10`)

| Offset | Size | Field | Meaning |
|-------:|-----:|-------|---------|
| `0x00` | 20 | `name` | NUL-padded name. |
| `0x14` | 4 | `offset` | byte offset of this node's region. |
| `0x18` | 4 | `length` | region length in bytes. |
| `0x1C` | 4 | `kind` | `0` = file, `1` = directory. |

Entry *i* lives at `16 + i*32`. File data conventionally begins right after the
table, at `16 + file_count*32`.

## What `mount` checks (and the one thing it doesn't)

The reader ([`Fs::mount`](../../src/fs.rs)) is *total* — every malformed image gets a
named error, never a panic:

- `BadMagic` — first four bytes aren't `ZRFS`.
- `TotalSizeMismatch` — `total_size` != the real image length.
- `EntryOutOfBounds` — some entry's `offset + length` runs past the image.
- `BadDataEnd` — `data_end` is itself out of range (`> total_size` or `< 16`).
- `ExtentEscapesData` — a **file** extent ends past `data_end` (Milestone 16's
  confinement: bytes past the declared data region — a planted secret — are
  unreachable through a normal entry).
- `DirNotAligned` / `BadKind` — a subdirectory length that isn't a whole number of
  32-byte entries, or a `kind` that's neither 0 nor 1.

The honest caveat, stated in `fs.rs` itself: **`data_end` lives *inside* the image,
and you supply the whole image.** `mount` range-checks `data_end` but cannot know
whether the *value* is honest — a valid-but-forged `data_end = total_size` passes
every check and reopens the leak. That is the entire lesson of the CTF, and it
generalizes: *validating against a number the caller supplies is not validation.*

## A worked example — an honest two-file disk

Two files, `notes.txt` (21 bytes) and `readme.md` (29 bytes). The table is
`2 × 32 = 64` bytes, so data starts at `16 + 64 = 80` (`0x50`):

```
offset  bytes                                   meaning
0x00    5A 52 46 53                             "ZRFS"
0x04    03 00                                   version = 3
0x06    02 00                                   file_count = 2
0x08    82 00 00 00                             total_size = 130   (80 + 21 + 29)
0x0C    82 00 00 00                             data_end   = 130   (= end of file bytes)
0x10    6E 6F 74 65 73 2E 74 78 74 00 …(20)     name "notes.txt"
0x24    50 00 00 00                             offset = 80
0x28    15 00 00 00                             length = 21
0x2C    00 00 00 00                             kind = file
0x30    72 65 61 64 6D 65 2E 6D 64 00 …(20)     name "readme.md"
0x44    65 00 00 00                             offset = 101       (80 + 21)
0x48    1D 00 00 00                             length = 29
0x4C    00 00 00 00                             kind = file
0x50    … 21 bytes of notes.txt …
0x65    … 29 bytes of readme.md …
```

`mount` accepts it; `ls /` lists both; `cat /notes.txt` prints 21 bytes. Nothing
reaches past `data_end`.

## A reference builder

This is the same serializer the CTF's **✎ Craft mode** uses. It builds *exactly*
what you pass — it doesn't choose an exploit for you:

```js
const LE16 = v => [v & 255, (v >> 8) & 255];
const LE32 = v => [0,8,16,24].map(s => (v >>> s) & 255);
const enc  = new TextEncoder();

// files: [{name, data:Uint8Array}], plus explicit superblock/dirent overrides.
function buildZranFS(files, { totalSize, dataEnd, entries }) {
  const DIRENT = 32, SB = 16;
  let off = SB + files.length * DIRENT;
  const ext = files.map(f => { const e = { off, len: f.data.length }; off += f.data.length; return e; });
  const imgLen = off;                                   // the real bytes we emit
  const buf = new Uint8Array(imgLen);
  buf.set(enc.encode('ZRFS'), 0);
  buf.set(LE16(3), 4);
  buf.set(LE16(files.length), 6);
  buf.set(LE32(totalSize ?? imgLen), 8);                // declare whatever you want
  buf.set(LE32(dataEnd   ?? off),    12);
  files.forEach((f, i) => {
    const base = SB + i * DIRENT, o = entries?.[i] ?? {};
    buf.set(enc.encode(f.name).slice(0, 20), base);
    buf.set(LE32(o.offset ?? ext[i].off),    base + 20);
    buf.set(LE32(o.length ?? ext[i].len),    base + 24); // declare a length past your data to reach further
    buf.set(LE32(0),                          base + 28);
  });
  files.forEach((f, i) => buf.set(f.data, ext[i].off));
  return buf;   // btoa(String.fromCharCode(...buf)) → paste into `load`
}
```

The levers the CTF turns on are all here: `dataEnd` (forge it), a directory entry's
`length` (stretch it past its own data), and `totalSize` (in the strict Tier-1
reader it must equal the real length; the [Tier-2 reader](../../web/ctf-remote.html)
relaxes *that* cross-check, which is what lets a crafted image reach a secret the
host appended past your bytes). **This primer stops at the format.** *What* to set,
and to which per-session offset, is the puzzle — rehearse it in the
[Tier-1 sandbox](../../web/ctf.html), where the flag is your own and you can
iterate, then reproduce it against the offset Tier 2 hands you.
