# Setting up to build nothing

*Milestone 0 — toolchain and scaffolding.*

The first commit of an operating system doesn't do anything. There's no boot, no
output, nothing to screenshot. The whole milestone is spent making it *possible*
to build something that does nothing — an empty kernel that compiles for a
machine that isn't your machine. It's the least glamorous part and it's where a
lot of OS projects quietly die, so it's worth writing down.

## The one decision that shapes everything else

Where do you get a bootloader? There are two honest paths:

1. **Hand-write the whole thing** — a real-mode boot sector, the jump to
   protected mode, the jump to long mode, all in raw x86 assembly. Maximum
   learning, maximum pain.
2. **Let an existing bootloader do the real-mode dance** and start the
   interesting kernel work sooner.

I split the difference, deliberately. The genuinely instructive part of booting
isn't the stage-1/stage-2 disk-loading choreography — it's the sequence that
*enters long mode*: checking the CPU can do it, building the first page tables,
setting PAE, setting the long-mode-enable bit in EFER, and flipping on paging.
That part I hand-wrote and understand line by line. The part that just loads
bytes off a disk, I handed to GRUB via a **Multiboot2** header. The raw boot
sector is still a fair exercise later — it's just not allowed to block every
milestone after it. That decision is recorded in the plan so I don't relitigate
it at 1 a.m. three weeks from now.

## The toolchain

- **Rust, `no_std`, on stable.** The bare-metal target `x86_64-unknown-none` is
  Tier-2 and ships a precompiled `core`, so there's no `build-std` and no nightly
  needed yet. Lower barrier to a reproducible build = more likely I actually keep
  going. `rust-toolchain.toml` pins it so a fresh clone and CI agree.
- **`nasm` + `ld`** for the boot assembly and the final link.
- **QEMU** as the primary debug loop; **GDB** for when it triple-faults and the
  machine just… reboots, with no stack trace. (That reboot-with-no-explanation is
  the thing everyone warns you about. It's real.)

## The shape of the build

No `bootimage`, no clever `build.rs`. The `Makefile` spells out the entire
pipeline so I can read how the sausage is made:

```
nasm  boot/*.asm          -> build/*.o
cargo build (x86_64-none) -> libziran_kernel.a
ld    (linker.ld)         -> build/kernel.bin     (a Multiboot2 kernel)
grub-mkrescue             -> build/ziran.iso
qemu-system-x86_64        -> boots it
```

The kernel is compiled as a **static library** and linked *into* the boot
assembly, rather than the other way around. The assembly is the true entry
point (`_start`); it eventually calls the `kernel_main` symbol exported from the
Rust staticlib. The linker script's only real jobs are to put the Multiboot2
header first (GRUB scans the first 32 KiB for it) and to load everything at the
1 MiB mark, safely above the legacy memory below.

## The check that made it feel real

Before anything boots, there's a cheap way to know you didn't fumble the
Multiboot2 header: the first four dwords of the image must sum to zero, mod 2³².
It's the boot protocol's built-in checksum. When `readelf -x .boot` showed the
magic `0xe85250d6` at the very start of the loaded image and the arithmetic
closed to zero, that was the first real signal — the bytes are in the right
place, in the right order, and a bootloader will recognize them.

## What CI actually asserts

The most useful thing a young OS repo can have is a single honest signal: *does
it still boot?* Every push assembles, compiles, links, builds an ISO, and boots
it under a **headless QEMU**, then greps the serial output for the marker the
kernel prints once it reaches long mode. Not "does it compile" — compiling is
easy. "Does it still reach 64-bit Rust code." That's the line worth defending.

Next: the first 512 bytes — what the boot assembly actually does before Rust
gets to run at all.
