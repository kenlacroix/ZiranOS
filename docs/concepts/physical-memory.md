# Physical memory, from first principles

*Companion to Milestone 6. Read this before `src/multiboot.rs` and
`src/frame_allocator.rs` — it explains what those files are *for* and why the
job is trickier than "just start using RAM." Builds on the boot path from
[interrupts.md](interrupts.md) only loosely; the one thing you need is the idea
that after `boot/boot.asm` we are running in 64-bit long mode with the first
1 GiB of memory identity-mapped.*

---

## The problem: the kernel is blind to its own RAM

Here is a fact that surprises everyone the first time: when `kernel_main` starts
running, **the kernel has no idea how much RAM the machine has.** It does not
know if there is 128 MiB or 128 GiB. It does not even know which physical
addresses are backed by real memory chips.

Why not just poke around and find out? Because physical address space is not a
flat field of RAM. It is a patchwork. Some ranges are real, writable DRAM. But
others are:

- **Reserved by firmware** — the BIOS/UEFI keeps tables and code down low.
- **Memory-mapped devices** — writing to an address there talks to *hardware*,
  not memory. The VGA text buffer we scribble on lives at `0xb8000`, smack in
  the middle of the legacy hole `0xA0000–0xFFFFF`. That whole region is device
  and BIOS space, not RAM.
- **Holes** — plain gaps where nothing is wired up at all.

If the kernel guessed "RAM is a contiguous block from 0 to N" and started using
it, it would eventually write into the VGA region (garbling the screen), or into
a device register (undefined behaviour), or into a hole (a fault, or a silent
loss). It cannot probe its way out of this safely either — reading a device
register can have side effects.

So the kernel must be **told**. Someone who surveyed the machine *before* our
code ran has to hand us a map.

## Who tells it: the Multiboot2 boot information structure

That someone is the bootloader — GRUB, in our case. GRUB talks to the firmware,
collects the memory layout (the classic BIOS "e820" map, among other things),
and packages it into a **Multiboot2 boot information structure**. It leaves that
structure sitting in low RAM and hands us a pointer to it.

We already caught that pointer. Back in `boot/boot.asm`, at the very top of
`_start`:

```asm
    ; EBX holds the physical address of the Multiboot2 info structure. Stash it
    ; in EDI so it survives into long mode as the first C-ABI argument...
    mov edi, ebx
```

GRUB leaves the address in `EBX`; we copy it to `EDI` so it survives the switch
to long mode. In `boot/long_mode_init.asm`, `EDI` widens to `RDI`, and `RDI` is
where the System V C ABI passes a function's first argument. So the moment
`kernel_main` grows a parameter, the pointer arrives for free:

```rust
pub extern "C" fn kernel_main(multiboot_info_addr: u64) -> !
```

As of Milestone 6 that signature is in place. The assembly needed no change at
all: `boot/boot.asm` already stashed GRUB's pointer in `EDI` (with an eye to this
day), so it was waiting in `RDI` the whole time — growing the Rust parameter was
enough to catch it.

## Reading the structure: why it's a chain of tags

You might expect the boot information to be a fixed C struct you cast a pointer
to and read fields from. It isn't, and the reason is worth understanding because
the same idea recurs all over systems programming.

Multiboot2 is **extensible and self-describing**. GRUB and the kernel are built
separately, possibly years apart. A fixed struct would freeze the format
forever: the day someone adds a new field, every old kernel misreads every field
after it. So instead the structure is a **list of tags**, each announcing its
own type and size:

```
+0   total_size : u32      (whole structure, in bytes)
+4   reserved   : u32      (0)
+8   ┌─ tag ─────────────┐
     │ type : u32         │   what this tag is (6 = memory map)
     │ size : u32         │   bytes in THIS tag, incl. these 8
     │ ...payload...      │
     └───────────────────┘
     (padding to the next 8-byte boundary)
     ┌─ tag ─────────────┐
     │ ...               │
     ...
     ┌─ end tag ─────────┐
     │ type = 0, size = 8│   stop here
     └───────────────────┘
```

A parser walks this by starting at offset 8, reading a tag's `type` and `size`,
acting on it if it recognises the type, then striding forward to the next tag —
and *stopping* at the end tag (`type == 0`). An unknown tag type is simply
skipped by its `size`. That is what "self-describing" buys you: forward and
backward compatibility, for the price of a loop instead of a struct cast.

**The alignment gotcha.** Every tag is padded so the *next* tag starts on an
8-byte boundary. A tag's `size` field does **not** include that padding, so you
must round up when advancing: `next = (here + size + 7) & !7`. Forget it and
your walk drifts out of alignment and reads garbage for every tag after the
first odd-sized one.

## The tag we came for: the memory map (type 6)

Among the tags is the one that answers our original question — **tag type 6, the
memory map.** Its payload is a small header followed by a run of fixed-size
entries:

```
type          : u32   = 6
size          : u32
entry_size    : u32   <- bytes per entry (authoritative!)
entry_version : u32
  entry[0] : { base_addr : u64, length : u64, kind : u32, reserved : u32 }
  entry[1] : { ... }
  ...
```

Each entry describes one contiguous physical region: where it starts
(`base_addr`), how big it is (`length`), and what it is (`kind`). The kind we
care about is **`kind == 1`, meaning "usable RAM."** Everything else — reserved,
ACPI, bad memory — we leave strictly alone.

**The second gotcha, and it is a real corruption bug:** stride between entries by
`entry_size` from the header, **not** by the size of the struct you wrote in
Rust. The spec explicitly allows a loader to make entries *larger* than the
24-byte `{base, length, kind, reserved}` we know about, padding them for future
fields. If you hardcode 24 and the loader used 32, every entry after the first
is misread. The header hands you the stride on purpose — use it.

This split — a dumb reader that just yields raw `(base, length, kind)` entries —
is exactly what `src/multiboot.rs` is: a read-only parser. It borrows GRUB's
structure in place (it sits in identity-mapped low RAM, so we can read it
directly) and does not decide policy. Policy is the allocator's job.

## What a "physical frame" is

Now we know which physical address ranges are real RAM. But RAM handed out as
raw byte ranges of odd sizes would be miserable to manage. So, universally,
kernels chop physical memory into fixed-size chunks called **frames** (or "page
frames"). A frame is **4 KiB** of contiguous physical RAM, aligned to a 4 KiB
boundary. Physical address `0x0` starts frame 0; `0x1000` starts frame 1; and so
on.

Why 4 KiB specifically? Because that is the page size the x86-64 MMU uses. When
Milestone 7 turns on real paging, the hardware maps memory one 4 KiB page at a
time, and each mapping consumes one physical frame to hold RAM (and more frames
to hold the page tables themselves). Making the allocation unit match the
hardware's mapping unit means there is never a mismatch to paper over. The frame
is the atom that everything above — paging (M7), the heap (M8) — builds on.

A tiny newtype makes this a *type*, not a bare number: `PhysFrame(u64)` holding a
4 KiB-aligned physical address. That way a frame can never be silently confused
with an ordinary integer or a virtual address.

## The frame allocator: tracking free vs. used

The **frame allocator** is the bookkeeper. Its whole job:

- `alloc() -> Option<PhysFrame>` — hand out a free frame, marking it used.
- `free(frame)` — take a frame back, marking it free again.

`Option` because RAM is finite: when nothing is free, `alloc` returns `None`, and
the caller must cope rather than pretend it got memory.

The design chosen for Ziran (see the Milestone 6 plan) is a **bitmap**: one bit
per 4 KiB frame, where `0 = free` and `1 = used`.

```
frame #:   0    1    2    3    4    5    6    7   ...
bitmap:  [ 1  | 1  | 0  | 0  | 1  | 0  | 0  | 0 ] ...
           ^kernel^   ^free^  ^^   ^^^^free^^^^
```

`alloc` scans for the first `0` bit, sets it, and returns that frame. `free`
clears the bit. The bitmap lives in a fixed, statically reserved buffer in
`.bss` (part of the kernel image) sized for a generous maximum — so we sidestep
the chicken-and-egg of needing an allocator to allocate the allocator's own
storage.

**Why a bitmap and not something simpler?** The obvious first allocator is a
*bump* allocator: keep a "next free frame" pointer and hand out the next one each
time. It is one line. But it **cannot reclaim** — there is nowhere to record that
a freed frame is available again, so `free` is a no-op and memory leaks forever.
Milestone 6's stated goal is an allocator that *hands out and reclaims*, so the
bump allocator is out. A bitmap gives O(1) `free`, makes double-frees cheap to
catch (the bit is already clear), and — not least — is the most *teachable*
shape there is: it is literally a picture of which pages are taken.

**Why not an intrusive free-list?** A free-list threads a linked list through the
free frames themselves, storing each "next" pointer *inside* the free frame. That
is elegant and O(1), and it is what a mature kernel often uses. But it requires
**writing into a frame to free it** — and right now we can only touch memory the
CPU can reach, which is the first 1 GiB that `boot/boot.asm` identity-mapped.
A free frame above 1 GiB is real RAM the allocator can *know about*, but we
cannot write a pointer into it until paging (M7) maps it. Storing metadata
*outside* the frames — in the bitmap — sidesteps that entirely. The free-list
becomes sound once M7 exists; until then it is a footgun.

## The bootstrapping subtleties

The dangerous part of a frame allocator is not handing out memory — it is
*correctly* not handing out memory that is already spoken for. If `alloc` ever
returns a frame that is secretly in use, the bug is catastrophic and delayed:
something later overwrites live data, and the crash lands nowhere near the cause.

So initialisation is done defensively, in layers:

1. **Start with everything marked used.** The safe default is "nothing is free
   until proven free." Never hand out the unknown.
2. **Mark usable regions free.** For each memory-map entry with `kind == 1`, mark
   its frames free — rounding `base` *up* and `end` *down* to 4 KiB boundaries,
   so a partial frame straddling a reserved edge is dropped rather than half
   handed out.
3. **Re-reserve the frames we must never touch**, even though the map called
   them usable:
   - **Frame 0 (the null frame).** Physical address 0 is indistinguishable from
     a null pointer and sits in the legacy low-memory minefield. Never hand it
     out.
   - **The kernel image itself.** Our code, data, and `.bss` are loaded at 1 MiB,
     inside a region the map reports as usable. Handing out those frames would
     let the allocator give away the kernel's own guts. The linker exports
     `kernel_start` / `kernel_end` symbols so we can carve out exactly
     `[kernel_start, kernel_end)`.
   - **The Multiboot2 structure.** GRUB placed it in usable RAM too. If we don't
     reserve `[mb_addr, mb_addr + total_size)`, we could hand out the frames
     holding the very map we just parsed.

Miss any one of these and the allocator will cheerfully hand out live memory.

### "Can track but not touch"

One nuance deserves its own emphasis, because it is the seam between this
milestone and the next. The allocator may **record** a frame above the 1 GiB
identity map as free real RAM — that is honest bookkeeping. But nobody can
**read or write** that frame yet, because no page maps its physical address into
something the CPU can address. *Tracking* a frame and *using* a frame are
different powers, and until Milestone 7 builds paging we only have the first.

For Milestone 6 we sidestep the hazard entirely by running QEMU with `-m 128M`,
well under 1 GiB, so every frame the allocator hands out is inside the identity
map and safe to use immediately. RAM beyond the bitmap's coverage is *not*
silently dropped — the house rule is no silent caps, so anything ignored is
reported over serial, loud and visible.

## Where this goes next

- **Milestone 7 (paging)** is the first customer. Building a page table means
  allocating physical frames to *hold* those tables — so paging literally cannot
  start until a frame allocator exists. Paging is also what finally lets us
  touch frames above the identity map.
- **Milestone 8 (the heap)** sits on top of paging: `Vec`, `Box`, and `String`
  need a heap, the heap needs pages, and those pages are backed by frames from
  right here. Every dynamic allocation in the kernel's future traces back to a
  bit flipped in this bitmap.

That is the shape of the memory stack: **frames (M6) → pages (M7) → heap (M8).**
This milestone lays the bottom stone — teaching the kernel, for the first time,
what memory it actually has.
