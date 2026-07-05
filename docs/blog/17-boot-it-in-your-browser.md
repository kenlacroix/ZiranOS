# You can boot it in your browser now

*The real 64-bit kernel — not a screenshot, not a screen recording — running live
in a tab you can click. What that took, and the two bugs that had been hiding in
plain sight until I moved the specimen to a new enclosure.*

Almost every hobby OS you've ever seen, you've seen *behind glass*. A screenshot in
a README. A pressed flower in a book — flat, dead, beautifully arranged, and
utterly incapable of telling you whether the thing was alive when it was picked. You
look at it. You nod. You close the tab. You have no idea whether the shell echoes,
whether `ls` returns, whether it would fall over on the second command, because a
photograph of a running kernel and a running kernel share exactly one property: they
both look like a running kernel.

I wanted mine to be a living specimen in a terrarium. One click, and the actual
kernel boots — in *your* browser, on *your* machine, and you type at it. Not a
recording of me typing. You.

> **The lesson, up front: the moment you run your code on a new path, it stops
> being your code and becomes a stranger's. The paths you never exercise are
> holding bugs for you, interest-free, until the day you exercise them.**

## Why this is hard (and why the easy answer is a lie)

The obvious move is v86 — the wonderful little x86 emulator that already runs
Linux and Windows 98 in browser tabs across the internet. I tried it first. It dies
at `check_long_mode` with a terse `ERR: L` and never prints another byte.

Because v86 is **32-bit**. No `EFER`, no `CPUID 0x80000001`, no REX prefixes — no
long mode, ever. And Ziran is a 64-bit kernel; long mode is the first thing it
reaches for. I surveyed the rest of the lightweight browser emulators — Halfix,
Bochs-WASM, a couple of bespoke x86-64 cores — and they all failed the same class
of test: either no long mode at all, or long mode too incomplete to deliver a fault
through the IDT, or a CPU core with none of the VGA/PIC/PIT/PS2 hardware my kernel
actually drives. A 64-bit bare-metal kernel is a demanding houseguest.

So the "browser OS" demos you've seen are, almost without exception, 32-bit. To
boot a *real* 64-bit kernel in a tab, you need a real PC emulator with real long
mode. There is exactly one that fits: **QEMU itself, compiled to WebAssembly** —
the `ktock/qemu-wasm` project, `qemu-system-x86_64` run through Emscripten. It is
not a reimplementation. It is QEMU. The same QEMU I've been booting from the
`Makefile` this whole time, transplanted into the tab.

That's the honesty that matters here: the live boot is **real QEMU, real long mode,
real serial shell**. When you type `help` and it answers, a genuine x86-64 emulator
executed genuine kernel instructions to produce that answer. There's a lighter
"faithful reconstruction" tour next to it for people who don't want to download
~15 MB, and a genuine recorded serial session for people who want the fast version
— and all three are *labelled*, so you always know whether you're looking at the
living thing or a good drawing of it.

## Moving the specimen revealed what the old enclosure was hiding

Here's the part I didn't expect. Bringing up qemu-wasm didn't just add a deployment
target — it surfaced **two latent boot bugs that had been in the kernel for
milestones**, silently, because the normal boot path happened to paper over both of
them.

The normal path is ELF-via-GRUB: GRUB loads the kernel as an ELF image, honors the
program headers, does a pile of quiet housekeeping on the way in. The browser path
is different. qemu-wasm boots the kernel *flat*, via QEMU's `-kernel`, from a ~0.3 MB
`kernel-v86.bin` — a raw `objcopy -O binary` dump, no ELF, no GRUB, none of the
housekeeping. Same instructions, colder start.

**Bug one: the unloaded GOT.** The flat `-kernel` path never loaded the global
offset table that the ELF path had been loading for me all along. On the ELF path,
relocations were resolved and the GOT was populated before my code ran, so every
GOT-relative reference just worked. On the flat path there was no such courtesy —
the GOT sat there unloaded, and the first reference through it read garbage. This
had *never* been a bug, because I had *never* run without the ELF loader doing the
work. The bug was real the whole time; it was just always someone else's problem
until it was mine.

**Bug two: SSE wasn't explicitly enabled.** The kernel had been relying on SSE being
in a usable state without ever turning it on itself — again, fine under the normal
path and its accumulated CPU setup, but the wasm/TCG path required the kernel to
enable SSE explicitly rather than inherit it. Under TCG in the browser, "I assumed
it was on" became "`#UD` on the first SSE instruction."

Both are the same shape of bug, and it's the shape this whole post is about: code
that was correct *relative to a path I always took*, and incorrect the instant I
took a different one. Neither was a browser bug. The browser just declined to keep
my secrets.

### The scheduler that runs out of time

There was a third, gentler surprise — not a bug, a physics problem. Ziran has a
preemptive scheduler with a boot-time self-test that asserts a context switch lands
inside a specific timing window. Under qemu-wasm's browser timing, wall-clock time
is *dilated* — the emulator, the WASM layer, the browser's own scheduling all
stretch the seconds — and the self-test simply can't hit its window anymore. The
switch happens; the clock it's measured against is molasses.

The honest fix was not to loosen the test everywhere (that would blind it on the
real path, where it's a genuine invariant). It was to make the self-test detect the
dilated environment and **warn instead of panic** there. A hard `panic!` on the
metal, a logged warning in the tab. The invariant still holds where it can be
trusted; where the clock is unreliable, the test says so out loud instead of taking
the whole kernel down over a stopwatch it can't read.

## The deploy bug that shipped a ghost

Now the war story I'm least proud of, which is exactly why it's here.

`kernel-v86.bin` — the flat binary the browser loads — is a **build artifact, and
it's gitignored.** Sensible: you don't commit compiled output. But it means the
file the deploy uploads is whatever happens to be sitting in `web/` at deploy time,
and *nothing about the source tree forces it to be current*.

So at one point I deployed, opened the live site, and typed `load` — a command that
exists in my source — and the kernel told me it had never heard of it. I had shipped
a **months-old kernel**. A stale `kernel-v86.bin` from some earlier experiment was
still in the folder; the deploy dutifully uploaded the ghost while my actual, current
source sat one `objcopy` away, untouched. The site booted a real kernel. It just
wasn't *this* kernel. A living specimen, yes — of the wrong animal.

This is the sneakiest failure mode in the whole project, because everything works.
It boots. It's interactive. It's real QEMU running real long mode. It is simply the
*wrong bytes*, and there is no error message for "the right bytes, but old."

The fix is boring and correct: a deploy target that **rebuilds the kernel first**,
so shipping stale is not a thing you can do by forgetting. `make redeploy-web`
depends on `stage-web-kernel` (a fresh `objcopy` from the current build) before it
runs the upload. The plain `deploy-web` still exists for the wasm assets, but the
safe default regenerates the artifact it's about to publish. The gitignore stays;
the discipline moves into the Makefile, where forgetting can't reach it.

I lost more time to this than to the GOT and SSE bugs *combined*, and none of it
was interesting time. It was staring-at-a-correct-looking-boot-wondering-why-`load`-
doesn't-exist time. The bugs that hurt most are the ones with no stack trace.

## Hosting: the boring constraints that decide everything

The live boot runs on **Cloudflare Pages**, and the reasons are unglamorous and
load-bearing:

- **Cross-origin isolation is mandatory.** Emscripten's pthreads use
  `SharedArrayBuffer`, which the browser only hands you when the document is
  cross-origin isolated — meaning it *must* be served with
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp`. Plain GitHub Pages can't set those
  headers. Cloudflare Pages can, via a `_headers` file. That single limitation is
  the entire reason for the host choice.
- **The wasm is ~15.5 MB** — under Pages' 25 MB-per-file cap, so a plain direct
  upload works. No object storage, no R2, no signed URLs. (An earlier plan budgeted
  for a ~46 MB artifact and assumed I'd need R2; trimming the build under the cap
  deleted an entire tier of infrastructure. The best deploy architecture is the one
  you didn't have to build.)
- **The kernel loads as ~0.3 MB flat, not the 16 MB ISO.** The browser path takes
  `kernel-v86.bin` via `-kernel`, not the full bootable image. Smaller payload,
  faster boot, and — as above — its own footgun.

## What I deferred and didn't verify

Honesty tax, paid in full:

- **The live boot is browser-only to verify.** There is a headless-Chrome smoke
  test (`make web-test`) that confirms it boots and the serial shell answers, but
  the *experience* — latency, feel, whether it's pleasant rather than merely
  functional — I've checked by hand in a few browsers, not systematically across
  the matrix. I have not load-tested it, and I don't know how it behaves on a phone.
- **The scheduler self-test's warn-don't-panic branch trusts an environment sniff.**
  If that detection is ever wrong, it would warn on real hardware where it should
  panic. The window's the same; only the reaction changed — but a test that changes
  its mind based on where it thinks it's running is a test I'll want to revisit.
- **The build is heavy and I did not make it light.** Compiling QEMU to WASM is a
  slow Docker-and-Emscripten affair; on Apple Silicon it runs under emulation and
  can fall over on memory pressure. I documented "use a native x86_64 Linux box"
  rather than fixing the toolchain to be pleasant on a Mac. That's a papered-over
  path, and by this post's own thesis, papered-over paths are just bugs I haven't
  met yet.

## Go click it

That's the whole point, and it's a small one: the difference between *looking at* a
hobby OS and *using* one is a single click, and almost no hobby OS is clickable. The
pressed flower and the living thing look identical on the page. Only one of them
answers when you type at it.

- **Boot the real kernel:** <https://ziranos.pages.dev>
- **Break the filesystem (the CTF):** <https://ziranos.pages.dev/ctf>

The CTF is where the "living specimen" framing earns its keep. Tier 1 is white-box —
the whole trick is in front of you: the flag sits in the RAM disk with no directory
entry naming it, and you craft a disk whose file entry *aliases* the secret's bytes
so the reader hands you what navigation never could. Offsets instead of pointers; a
bounds check against the wrong bound. Tier 2 is the same family of mistake — a
length-confusion bug, with the flag living only in the server's RAM, injected fresh
per session so there's nothing to leak but the thing you have to earn — but I'm
deliberately not writing the recipe down, because a solved CTF is a dead one, and
I'd rather leave you a living one.

(All of this was built openly with Claude Code. The AI is the workshop; the engineering
— the GOT that wasn't loaded, the SSE that wasn't on, the ghost kernel I shipped by
forgetting — is the story. Tools don't have war stories. People do.)
