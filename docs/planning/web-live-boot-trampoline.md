# Web teaching tool — making the live boot real (v86 Multiboot1 trampoline)

Captures a recon finding so it isn't lost: **why the browser tool's "live mode"
never actually booted the kernel, and the concrete fix.** Verify the fix in a
real browser (needs iteration you can't do headlessly), so this is post-update
work — the plan and the wiring are ready for it.

## The finding (verified by reading v86's `libv86.js`, 0.5.420)

v86 **cannot load our kernel image at all**, for two independent reasons:

1. **v86 only recognizes Multiboot _1_.** Its loader
   (`load_multiboot_option_rom`) scans the first 8 KiB for exactly the MB1 magic
   `0x1BADB002`. It never looks for the Multiboot _2_ magic `0xe85250d6` (grep of
   the source: 0 hits). Our image is MB2-only, so v86 finds nothing and boots the
   BIOS to a blank machine.
2. **Its ELF path is 32-bit only.** Even when the MB1 magic is found, the ELF
   branch asserts `class == 1` (ELF32), `ehsize == 52`, `phentsize == 32`. Our
   image is ELF64 (`class 2`, ehsize 64, phentsize 56). v86 hands off in 32-bit
   protected mode (`is_32=1`, EIP = `e_entry`), so a 64-bit entry can't be used.

Consequence: the page's "live mode" **silently fell back to the reconstruction
every time** — which is exactly why it "felt like a movie." (The wrong pinned CDN
URL, `v86@0.5.228`, guaranteed the fallback too, but even with correct assets it
could not have booted.) The page is now honest about this: it shows a labelled
reconstruction and only ever claims "live" if the real serial marker appears.

## The fix: a 32-bit Multiboot1 trampoline

Give v86 something it can actually load — a **Multiboot1, 32-bit** entry that
then enters long mode and jumps to the existing ELF64 kernel. Two routes:

- **(A) MB1 a.out kludge (flat binary).** Add an MB1 header (magic `0x1BADB002`,
  flags with bit 16 set, valid checksum) carrying explicit
  `header_addr`/`load_addr`/`load_end_addr`/`bss_end_addr`/`entry_addr`. v86 then
  copies the flat payload to those physical addresses and jumps to `entry_addr`
  in 32-bit mode. No reliance on ELF parsing.
- **(B) 32-bit ELF trampoline (recommended for readability).** A tiny separate
  32-bit stub object with the MB1 header and a 32-bit `_start` that: re-checks
  it's fine, sets up (or reuses) the long-mode transition, and jumps to the
  ELF64 kernel's `_start`. Keep it a *dual-header* image so **GRUB still uses
  MB2** and **v86 uses MB1** — both headers can coexist in the first 32 KiB.

Either way the real 64-bit kernel is unchanged; the trampoline is the shim.
Note our existing `boot/boot.asm` already does the 32-bit→long-mode ritual, so
much of the trampoline logic exists — the new part is the MB1 header + making
the very first entry 32-bit-loadable by v86.

### Watch-outs

- v86's guest memory: the page config uses `memory_size: 64 MiB` — the M6 memory
  map the kernel reports will reflect *that*, not QEMU's 128 MiB. Keep the two in
  mind when the tour explains the map.
- v86 passes the MB info pointer in **EBX** (32-bit), same as GRUB — the
  `EBX→EDI→RDI` stash in `boot.asm` still applies.
- Confirm the RWX-segment / load-address layout v86 expects matches `linker.ld`
  (load at 1 MiB).

## Verification (in a browser — the part that needs eyes)

1. Build with the trampoline, `make web`, `cd web && python3 -m http.server 8000`.
2. Open `localhost:8000`; the page attempts a real boot (once `LIVE_READY=true`).
3. Success = the kernel prints `Ziran OS booted…` over serial, the page's
   `serial0-output-byte` listener sees it, and the badge flips to
   **"live · real kernel"** on its own. If it doesn't appear, it stays a labelled
   reconstruction — no false "live".

## Wiring already in place (web/index.html)

- Assets self-hosted next to `index.html` (`libv86.js`, `v86.wasm`,
  `seabios.bin`, `vgabios.bin`) — no CDN dependency.
- `LIVE_READY` flag (currently `false`) gates the real-boot attempt. **Flip to
  `true` once the trampoline boots in a browser.**
- Honest live-detection: `realBoot()` only shows "live" after the real serial
  marker; otherwise `simBoot()` explains it's a reconstruction and why.
- Correct v86 0.5.420 API (`window.V86`, `wasm_path` string, `serial0-output-byte`).

## Then — the realness follow-ons (only meaningful once live)

- **Live LEDs:** no per-IRQ event exists in v86; approximate a "run/idle" light
  from `cpu-event-halt` and `instruction_counter[0]` deltas.
- **Register panel:** read `emulator.v86.cpu` live views
  (`instruction_pointer[0]`, `reg32[0..7]`, `cr[0..7]`, `flags[0]`) into a small
  updating panel — the one thing a "movie" can't fake. (Note: only 32-bit
  register views are exposed, even in long mode.)
