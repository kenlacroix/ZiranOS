# Restoring the build environment after a macOS major update

This is the "I just updated to macOS Tahoe 26 and my toolchain may be in
pieces" guide. Ziran OS is a bare-metal x86-64 hobby kernel, so its build chain
leans on a pile of cross tools that a major OS bump can knock over. This doc
gets you back to a green build without guesswork.

The short version: **the Rust half survives OS updates, the Homebrew half is the
part that breaks, and GRUB used to be the one genuinely painful dependency on
macOS — but on macOS 26 it is now a clean `brew install`.** There are two build
paths and only one of them needs GRUB — pick the path you actually need before
you go chasing tools you don't.

> **Update (macOS 26 / Tahoe, verified 2026-07):** native Path B now works.
> Homebrew ships a usable `x86_64-elf-grub` and a prebuilt `qemu` bottle, and the
> kernel boots under QEMU's bundled edk2 (UEFI) firmware. No Linux container, no
> building GRUB from source. See **Path B — native macOS** below; the container
> route is kept only as a fallback. The one wrinkle: Homebrew's `x86_64-elf-grub`
> is UEFI-only, so QEMU must be pointed at edk2 firmware with `QEMU_FIRMWARE=uefi`
> (the Makefile handles the details).

---

## The two build paths (read this first)

Ziran has two ways to boot, and they have very different dependency footprints.
Knowing which one you need saves you the worst of the setup.

| | **Path A — browser / teaching tool** (`web/`) | **Path B — ISO + QEMU** (`make iso` / `run`) |
|---|---|---|
| Produces | `build/kernel.bin` (raw Multiboot2 kernel) | `build/ziran.iso` (bootable CD image) |
| Boots via | [v86](https://github.com/copy/v86) in a browser tab | `qemu-system-x86_64` |
| Needs | cargo + nasm + GNU cross-linker | Path A **plus** `grub-mkrescue` **plus** `qemu` |
| GRUB required? | **No** | **Yes** (this is the hard one on macOS) |
| QEMU required? | **No** | **Yes** |
| Works natively on macOS today? | **Yes** | **Yes** on macOS 26 (`brew install x86_64-elf-grub qemu mtools coreutils`, boot under UEFI). Container is now just a fallback. |

If your goal is "see the kernel boot and poke at it," **Path A is enough and it
works on a stock Mac.** You only need Path B for the QEMU-based boot tests, GDB
debugging, or producing a real bootable ISO. Don't install GRUB unless you're
doing Path B.

v86 boots the raw Multiboot2 `kernel.bin` directly — it does **not** need an ISO
and does **not** need GRUB. That's the whole reason Path A dodges the painful
dependency.

---

## What's on the machine vs. what a macOS update takes out

**Survives an OS update (lives in `~/.cargo`, `~/.rustup`):**

- `rustc` + `cargo`
- rustup target `x86_64-unknown-none`
- the pinned toolchain from `rust-toolchain.toml`

**Homebrew-installed — may need a `brew reinstall` after a major bump, but the
formulae exist and are reliable:**

- `nasm` — assembles `boot/*.asm`
- `x86_64-elf-binutils` — the GNU cross binutils. Provides `x86_64-elf-ld`,
  `x86_64-elf-readelf`, `x86_64-elf-objcopy`, and friends. **This is what links
  the kernel** (see the linker gotcha below).
- `xorriso` — ISO filesystem tool that `grub-mkrescue` shells out to (Path B).
- `mtools` — provides `mformat`, which `grub-mkrescue` also shells out to when
  building the EFI boot image (Path B). Without it `grub-mkrescue` dies with
  `` error: `mformat` invocation failed ``.
- `coreutils` — provides `gtimeout` (macOS has no `timeout`), used by the headless
  boot test (Path B).

**Missing / needs installing:**

- `qemu-system-x86_64` — `brew install qemu` (Path B). The bottle also bundles
  the edk2/OVMF UEFI firmware the native boot needs.
- `grub-mkrescue` — on macOS 26 this **is** a simple `brew install x86_64-elf-grub`
  (it lands as `x86_64-elf-grub-mkrescue`). See Path B.
- `node` / `npm` — only needed for the optional headless v86 boot test; not part
  of the core build.

---

## The one gotcha that will waste your afternoon: the linker

macOS ships Apple's linker at `/usr/bin/ld`. **It cannot link this kernel.** It
rejects the GNU-style options the kernel needs and dies immediately:

```
ld: unknown option: -n
```

`linker.ld`, `-n` (nmagic), and `-T` are GNU `ld` semantics. Apple's `ld` speaks
a different dialect and there is no flag that makes it behave. Same story for
`readelf`, which doesn't exist on macOS at all.

The fix is to build with the GNU cross tools from `x86_64-elf-binutils`. The
Makefile honors `LD ?= ld` and `READELF ?= readelf` overrides, so on macOS you
build with:

```sh
make LD=x86_64-elf-ld READELF=x86_64-elf-readelf
```

Use that exact invocation everywhere below where a `make` target compiles or
links (`make`, `make check-header`, `make web`, `make iso`, ...). If you get
`ld: unknown option: -n`, you forgot the `LD=` override.

> Tip: if you're tired of typing it, set them in your shell —
> `export LD=x86_64-elf-ld READELF=x86_64-elf-readelf` — or add a
> `make`-wrapper alias. The Makefile only reads them as defaults, so an
> environment override works too.

---

## Restore-after-update checklist

Run these in order. Most are fast "is it still there?" checks.

### 1. Xcode Command Line Tools

A major macOS update frequently invalidates the CLT (Homebrew and everything
compiled from source depend on them).

```sh
xcode-select -p            # should print a path, e.g. /Library/Developer/CommandLineTools
xcode-select --install     # if the above errors or the tools are gone
```

If `xcode-select --install` says they're already installed but builds still fail
with missing headers, force a reinstall:

```sh
sudo rm -rf /Library/Developer/CommandLineTools
xcode-select --install
```

### 2. Homebrew health

```sh
brew --version
brew doctor
brew update
```

A major OS bump sometimes leaves formulae built against the old SDK. If tools
misbehave, reinstall the ones this project uses:

```sh
brew reinstall nasm x86_64-elf-binutils xorriso
```

Sanity-check the cross tools are on `PATH`:

```sh
which nasm x86_64-elf-ld x86_64-elf-readelf xorriso
```

### 3. Rust toolchain + target (should have survived)

```sh
rustc --version
cargo --version
rustup show                                   # confirms the rust-toolchain.toml toolchain
rustup target list --installed | grep x86_64-unknown-none
```

If the bare-metal target is missing:

```sh
rustup target add x86_64-unknown-none
```

### 4. QEMU + GRUB + helpers (Path B only)

```sh
brew install qemu x86_64-elf-grub mtools coreutils
qemu-system-x86_64 --version
which x86_64-elf-grub-mkrescue mformat gtimeout
```

- `qemu` — the emulator, plus the bundled edk2 UEFI firmware at
  `/opt/homebrew/share/qemu/edk2-x86_64-code.fd`.
- `x86_64-elf-grub` — provides `x86_64-elf-grub-mkrescue`. Note it ships **only**
  the `x86_64-efi` platform (no legacy `i386-pc`), so the ISO it builds is
  UEFI-only — which is why the boot targets pass `QEMU_FIRMWARE=uefi`.
- `mtools` — `mformat`, required by `grub-mkrescue`.
- `coreutils` — `gtimeout`, since macOS has no `timeout`.

### 5. node (optional — only for the headless v86 boot test)

```sh
brew install node
```

Skip this unless you're running the browser-side automated boot check. The core
build and the manual browser demo don't need it.

---

## Path A — boot in the browser (works on stock macOS)

This needs only cargo + nasm + the GNU cross-linker. No QEMU, no GRUB.

```sh
# 1. Build the kernel and stage it into web/ in one step:
make web LD=x86_64-elf-ld READELF=x86_64-elf-readelf
#    (equivalently: `make LD=... ` then `cp build/kernel.bin web/kernel.bin`)

# 2. Serve web/ over HTTP (v86 refuses file://):
cd web && python3 -m http.server 8000

# 3. Open http://localhost:8000
```

The v86 assets are **already self-hosted** in `web/` — `libv86.js`, `v86.wasm`,
`seabios.bin`, and `vgabios.bin` are checked in next to `index.html`, so this
works fully offline with no CDN. (See `web/README.md` for how the page picks
live vs. simulated mode and how those assets were sourced.)

If the browser boots to the kernel's output, Path A is healthy. Nothing else on
this page is required unless you specifically need QEMU or a real ISO.

---

## Path B — full ISO + QEMU

This is `make iso`, `make run`, `make run-headless`, and `make debug`. It needs
these beyond Path A: `qemu`, `x86_64-elf-grub`, `mtools`, `coreutils`
(checklist step 4).

### Path B — native macOS (recommended, macOS 26)

The old advice here was "don't fight GRUB natively, use a container." On macOS 26
that's no longer true — the native path is a handful of `brew install`s and one
extra QEMU flag. Here's why it works and how to run it.

**Why it works now:** Homebrew's `x86_64-elf-grub` gives a working
`x86_64-elf-grub-mkrescue`, and the `qemu` bottle ships prebuilt (no more failing
source compile against an old Clang). The one catch is that `x86_64-elf-grub`
ships **only the `x86_64-efi` platform** — it has no `i386-pc` (legacy BIOS)
modules — so the rescue ISO is **UEFI-only**. QEMU defaults to SeaBIOS (legacy
BIOS), which can't read that ISO; boot it fails over to network boot and you get
an empty serial log. The fix is to boot QEMU with the **edk2 (UEFI) firmware**
that's bundled with the `qemu` bottle. The Makefile does this for you when you
pass `QEMU_FIRMWARE=uefi`: it wires up the edk2 code + a writable copy of the
vars store as a `pflash` pair.

Because macOS also lacks `readelf`, `timeout`, and a GNU `ld`, the full native
invocation carries five overrides. Run any Path B target like this:

```sh
make run-headless \
  LD=x86_64-elf-ld \
  READELF=x86_64-elf-readelf \
  GRUB_MKRESCUE=x86_64-elf-grub-mkrescue \
  TIMEOUT=gtimeout \
  QEMU_FIRMWARE=uefi
```

Swap `run-headless` for `iso`, `run`, or `debug` as needed (same overrides). On a
green run you'll see GRUB load, then:

```
Ziran OS booted: kernel_main reached, long mode active.
...
[boot test] PASS -- kernel reached long mode
```

> That's a mouthful to type. Note it must be passed as **`make` arguments**, not
> shell exports: the Makefile assigns these with `:=`, and make only lets a
> command-line argument (not an environment variable) override a `:=` default. So
> wrap the whole invocation in a shell alias:
>
> ```sh
> alias zmake='make LD=x86_64-elf-ld READELF=x86_64-elf-readelf \
>   GRUB_MKRESCUE=x86_64-elf-grub-mkrescue TIMEOUT=gtimeout QEMU_FIRMWARE=uefi'
> # then: zmake run-headless   /   zmake iso   /   zmake debug
> ```

The `error: no suitable video mode found` and `WARNING: no console will be
available to OS` lines GRUB prints before the handoff are harmless — the kernel
talks over the serial port, not the GRUB console.

### Path B — Linux container (fallback)

If a future macOS update breaks the native GRUB again, or you want a build
environment immune to macOS entirely, run Path B inside a Linux container. A
Linux image has `grub-pc-bin`, `grub-common`, `xorriso`, `mtools`, and `qemu` as
one-line package installs, boots via legacy BIOS (no UEFI-firmware dance), and is
unaffected by macOS updates.

#### Option B1 — build/run inside a Linux container

With Rancher Desktop (or Colima/Lima) providing a `docker`- or `nerdctl`-
compatible runtime, run the full pipeline in a throwaway Debian/Ubuntu
container. Everything — including the linker — is native Linux inside, so you
**drop the `LD=`/`READELF=` overrides** (plain `ld`/`readelf` are the GNU ones
there):

```sh
# from the repo root
docker run --rm -it -v "$PWD":/src -w /src debian:bookworm bash -c '
  apt-get update &&
  apt-get install -y --no-install-recommends \
      build-essential nasm xorriso mtools \
      grub-pc-bin grub-common qemu-system-x86 \
      curl ca-certificates &&
  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y &&
  . "$HOME/.cargo/env" &&
  rustup target add x86_64-unknown-none &&
  make run-headless
'
```

(Swap `docker` for `nerdctl` if that's what your Rancher Desktop exposes. If you
do this often, bake those `apt-get`/`rustup` steps into a small `Dockerfile` so
each run doesn't reinstall the world.) `grub-pc-bin` supplies the BIOS-target
GRUB modules `grub-mkrescue` needs; without it you'll get an EFI-only or empty
image.

> **Building GRUB from source is no longer necessary.** Earlier versions of this
> guide walked through cross-building GRUB against an `i386-elf` toolchain (the
> phil-opp / OSDev route). On macOS 26 the Homebrew `x86_64-elf-grub` formula
> makes that obsolete — use **Path B — native macOS** above. The from-source
> route is only of historical interest now: OSDev wiki
> <https://wiki.osdev.org/GRUB>, phil-opp <https://github.com/phil-opp/x86_64-grub>.

### Which Path B target to run

Inside a Linux container (Option B1) everything is native GNU, so drop all the
overrides — just `make iso`, `make run-headless`, etc. Natively on macOS, carry
the five overrides from **Path B — native macOS** above:

```sh
# macOS-native; O = the override block, shown once for brevity:
#   LD=x86_64-elf-ld READELF=x86_64-elf-readelf \
#   GRUB_MKRESCUE=x86_64-elf-grub-mkrescue TIMEOUT=gtimeout QEMU_FIRMWARE=uefi
make iso          $O   # build build/ziran.iso
make run          $O   # boot in a QEMU window, serial->stdout
make run-headless $O   # no window; asserts the boot marker
make debug        $O   # freeze for GDB on :1234
```

(Or wrap them in the `zmake` alias shown above and run `zmake iso`, etc. These
must be `make` arguments, not shell exports — the Makefile's `:=` defaults only
yield to command-line overrides.)

---

## Verifying the restore

Work up from cheapest to most complete.

### 1. Kernel links + Multiboot2 header is valid (no QEMU/GRUB needed)

```sh
make check-header LD=x86_64-elf-ld READELF=x86_64-elf-readelf
```

Expected: `multiboot2 magic OK (d65052e8)`. This proves the cross-linker and
cross-readelf are working and the image is a well-formed Multiboot2 kernel. If
this passes, Path A is essentially guaranteed to work.

### 2. Browser boot (Path A)

Run the Path A steps and confirm the kernel's output appears in the browser at
`http://localhost:8000`. Live mode means v86 actually executed `kernel.bin`.

### 3. Headless QEMU boot test (Path B — needs qemu + GRUB)

```sh
make run-headless \
  LD=x86_64-elf-ld READELF=x86_64-elf-readelf \
  GRUB_MKRESCUE=x86_64-elf-grub-mkrescue TIMEOUT=gtimeout QEMU_FIRMWARE=uefi
```

This builds the ISO, boots it in QEMU under edk2 (UEFI) firmware with serial
captured to `build/serial.log`, and greps for the marker `Ziran OS booted`.
Success prints:

```
[boot test] PASS -- kernel reached long mode
```

That's the same check CI runs on every push. If it passes, both build paths are
fully restored.

---

## Troubleshooting

**`ld: unknown option: -n` (or `-T`, or any linker option)**
You're using Apple's `/usr/bin/ld`. Add the override:
`make LD=x86_64-elf-ld READELF=x86_64-elf-readelf`. If it still happens, confirm
`x86_64-elf-binutils` is installed and `which x86_64-elf-ld` resolves
(`brew reinstall x86_64-elf-binutils`).

**`readelf: command not found` / `check-header` fails weirdly**
macOS has no `readelf`. Pass `READELF=x86_64-elf-readelf` (it comes from
`x86_64-elf-binutils`).

**`grub-mkrescue: command not found`**
On macOS the binary is prefixed: `x86_64-elf-grub-mkrescue` (from
`brew install x86_64-elf-grub`). Pass `GRUB_MKRESCUE=x86_64-elf-grub-mkrescue`.
If you only need the browser demo, use **Path A**, which doesn't need GRUB at all.

**`` error: `mformat` invocation failed `` from grub-mkrescue**
`grub-mkrescue` needs `mformat` to build the EFI boot image. `brew install mtools`.

**Boot test FAILs with an empty `build/serial.log`; a VGA screendump shows
`Boot failed: Could not read from CDROM` then iPXE network boot**
The ISO built by Homebrew's `x86_64-elf-grub` is **UEFI-only** (that formula has
no `i386-pc`/legacy-BIOS platform), but QEMU defaulted to SeaBIOS, which can't
read it — so it fell through to network boot and the kernel never ran. Boot under
UEFI firmware by adding `QEMU_FIRMWARE=uefi` to the `make` line. (To inspect the
VGA console yourself: run QEMU with `-monitor unix:build/mon.sock,server,nowait`,
then `screendump build/screen.ppm` over that socket and `sips -s format png` it.)

**`qemu: could not load PC BIOS '.../edk2-x86_64-code.fd'` when you pass it to `-bios`**
The edk2 firmware is split into a read-only *code* half and a writable *vars*
half; it must be attached as two `pflash` drives, not via `-bios`. `QEMU_FIRMWARE=uefi`
does this correctly (code read-only on `unit=0`, a writable copy of the vars
template on `unit=1`).

**A downloaded emulator/app binary won't run — "cannot be opened because the
developer cannot be verified," or it's silently killed**
macOS quarantines files downloaded from the internet with the
`com.apple.quarantine` extended attribute. Strip it recursively:

```sh
xattr -dr com.apple.quarantine <path-to-file-or-app>
```

(Relevant if you re-download QEMU builds, v86 assets, or any helper binary after
the update rather than getting them via Homebrew.)

**`brew` commands fail or formulae behave oddly right after the update**
Run `brew doctor`, then `brew update`, then
`brew reinstall nasm x86_64-elf-binutils xorriso qemu`. A major OS bump can leave
bottles linked against the previous SDK.

**Rust builds fail with "can't find crate for `core`" or target errors**
The bare-metal target got dropped: `rustup target add x86_64-unknown-none`, and
confirm `rustup show` matches `rust-toolchain.toml`.

---

## TL;DR

1. `xcode-select --install`; `brew doctor && brew update`.
2. `brew reinstall nasm x86_64-elf-binutils xorriso` if anything's flaky.
3. Confirm Rust: `rustup show`, `rustup target add x86_64-unknown-none` if needed.
4. **Path A (browser, works today):**
   `make web LD=x86_64-elf-ld READELF=x86_64-elf-readelf`, then
   `cd web && python3 -m http.server 8000`.
5. **Path B (ISO/QEMU), native on macOS 26:**
   `brew install qemu x86_64-elf-grub mtools coreutils`, then
   ```sh
   make run-headless \
     LD=x86_64-elf-ld READELF=x86_64-elf-readelf \
     GRUB_MKRESCUE=x86_64-elf-grub-mkrescue TIMEOUT=gtimeout QEMU_FIRMWARE=uefi
   ```
   (`QEMU_FIRMWARE=uefi` is required because Homebrew's GRUB is UEFI-only.) A
   Linux container is now just a fallback if native GRUB ever breaks again.
6. Verify with
   `make check-header LD=x86_64-elf-ld READELF=x86_64-elf-readelf`
   and, for Path B, the `make run-headless ...` line above looking for
   `Ziran OS booted`.
