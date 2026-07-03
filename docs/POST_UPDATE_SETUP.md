# Restoring the build environment after a macOS major update

This is the "I just updated to macOS Tahoe 26 and my toolchain may be in
pieces" guide. Ziran OS is a bare-metal x86-64 hobby kernel, so its build chain
leans on a pile of cross tools that a major OS bump can knock over. This doc
gets you back to a green build without guesswork.

The short version: **the Rust half survives OS updates, the Homebrew half is the
part that breaks, and GRUB is the one genuinely painful dependency on macOS.**
There are two build paths and only one of them needs GRUB — pick the path you
actually need before you go chasing tools you don't.

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
| Works natively on macOS today? | **Yes** | Only via a container, realistically (see Path B) |

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

**Missing / needs installing:**

- `qemu-system-x86_64` — `brew install qemu` (Path B).
- `grub-mkrescue` — **not a simple `brew install`.** See Path B.
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

### 4. QEMU (Path B only)

```sh
brew install qemu
qemu-system-x86_64 --version
```

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
two more things beyond Path A:

- `qemu-system-x86_64` — easy: `brew install qemu` (checklist step 4).
- `grub-mkrescue` — **the genuinely painful one on macOS.** Read below before
  you sink time into it.

### The honest truth about grub-mkrescue on macOS

There is **no reliable, first-class Homebrew formula that gives you a working
`grub-mkrescue` for building x86 BIOS-boot rescue images.** Mainline Homebrew
does not ship a usable GRUB for this, and the `*-elf-grub` style taps are
inconsistently maintained and frequently break across macOS releases. GRUB is a
bootloader that has to be cross-built for the `i386-pc` target, and Apple's
LLVM/clang cannot produce the `i386-elf` binaries GRUB needs — so any native
route means building GRUB (and its `mtools`/`xorriso` helpers) from source with
a cross toolchain. It is fiddly, it breaks on OS updates, and it is exactly the
kind of yak-shave a major-version bump loves to reopen.

**Recommendation: don't fight GRUB natively. Run Path B inside a Linux
container.** That's why Rancher Desktop / Lima was being set up on this machine.
A Linux image has `grub-pc-bin`, `grub-common`, `xorriso`, `mtools`, and `qemu`
as one-line package installs that Just Work, and it's immune to macOS updates.

#### Option B1 (recommended) — build/run inside a Linux container

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

#### Option B2 (native, only if you insist) — build GRUB from source

If you truly want a native macOS `grub-mkrescue`, you build GRUB against an
`i386-elf` (or `x86_64-elf`) cross toolchain, plus `objconv`, plus `xorriso` and
`mtools`. This is the phil-opp / OSDev route and it is well-documented but
brittle across macOS versions:

- OSDev wiki: <https://wiki.osdev.org/GRUB> (macOS section)
- phil-opp's prebuilt-ish approach: <https://github.com/phil-opp/x86_64-grub>

Expect to install a cross binutils/GCC, `brew install objconv xorriso`, `brew
install mtools`, then `configure`/`make`/`make install` GRUB with
`TARGET_CC`/`TARGET_OBJCOPY`/etc. pointed at the cross tools. Budget an evening,
and expect to redo it after the next macOS update. **For a hobby project, B1 is
the better use of your life.**

### Running Path B (once GRUB + QEMU exist)

Natively on macOS, still with the linker overrides:

```sh
make iso          LD=x86_64-elf-ld READELF=x86_64-elf-readelf   # build build/ziran.iso
make run          LD=x86_64-elf-ld READELF=x86_64-elf-readelf   # boot in a QEMU window, serial->stdout
make run-headless LD=x86_64-elf-ld READELF=x86_64-elf-readelf   # no window; asserts the boot marker
make debug        LD=x86_64-elf-ld READELF=x86_64-elf-readelf   # freeze for GDB on :1234
```

Inside a Linux container (Option B1) drop the overrides — just `make iso`,
`make run-headless`, etc.

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
make run-headless LD=x86_64-elf-ld READELF=x86_64-elf-readelf
```

This builds the ISO, boots it in QEMU with serial captured to
`build/serial.log`, and greps for the marker `Ziran OS booted`. Success prints:

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
Expected on stock macOS. If you only need the browser demo, you're on the wrong
path — use **Path A**, which doesn't need GRUB. If you genuinely need an ISO, use
**Path B Option B1** (Linux container) or, reluctantly, B2 (build GRUB from
source).

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
5. **Path B (ISO/QEMU):** `brew install qemu`; run GRUB in a **Linux container**
   (`make run-headless` inside Debian) — don't fight native macOS GRUB.
6. Verify with
   `make check-header LD=x86_64-elf-ld READELF=x86_64-elf-readelf`
   and, for Path B, `make run-headless ...` looking for `Ziran OS booted`.
