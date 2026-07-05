# Launch copy — per audience

Ready-to-post announcement text for Ziran OS, tailored per channel. **Spoiler
rule:** never publish the Tier-2 exploit recipe — it's redacted on purpose so the
remote capture stays solvable. Promote it as a challenge; describe the *concept*
(a length-confusion filesystem bug; the flag lives only in the server's RAM), never
the step-by-step. Tier 1 is white-box, so walking through it is fine.

Canonical links:
- Live tour / browser boot: **https://ziranos.pages.dev**
- Tier-1 CTF (browser, practice): **https://ziranos.pages.dev/ctf**
- Tier-2 CTF (remote capture): **https://ziranos.pages.dev/ctf-remote**
- Source + build log: **https://github.com/kenlacroix/ZiranOS**

The through-line (say it once, everywhere): *a from-scratch x86-64 OS in Rust that
boots in your browser — and hosts a CTF against its own kernel.*

---

## Show HN

**Title** (pick one; keep it a verb the reader can do):
- `Show HN: A from-scratch OS in Rust that boots in your browser – now steal a flag from it`
- `Show HN: I hid a flag in my hobby OS's RAM. Boot it in your browser and take it.`

**Body:**
> Ziran OS is a hobby x86-64 operating system I built from scratch in `no_std`
> Rust with hand-written Multiboot2/long-mode assembly — bootloader, paging, heap,
> a preemptive scheduler, a shell, a filesystem, ring-3 userspace with syscalls.
>
> Two things make it worth a click rather than another "I wrote an OS" post:
>
> 1. **It boots in your browser.** Real QEMU compiled to WebAssembly runs the
>    *actual* 64-bit kernel to an interactive shell in a tab — not a recording.
>    There's a guided predict→observe→explain tour over every layer.
>
> 2. **There's a CTF against the kernel itself.** Tier 1 (in-browser, white-box) is
>    a practice sandbox: craft a filesystem image and make the FS hand you a flag
>    `ls` can't see. Tier 2 is a *real* remote capture — a unique flag is minted per
>    session and injected into that server instance's RAM. It's not in the page, the
>    repo, or any bytes you're given. Develop your exploit against Tier 1, then land
>    it over the wire to capture a flag you genuinely could not otherwise see.
>
> It's openly AI-assisted (Claude Code), but with the verification discipline that
> usually gets skipped: CI *boots* the kernel and asserts milestone markers, the
> filesystem's mount path is fuzzed against corrupt images, and every `unsafe`/asm
> change gets a staff-style review. The honest build log (what broke, how long it
> took, the actual fix) is in the repo.
>
> Non-goals on purpose: QEMU-only, no POSIX, no real hardware, no security
> *hardening* — the security work is about *understanding* boundaries by attacking
> your own kernel, not defending against the world.
>
> Boot it: https://ziranos.pages.dev · CTF: https://ziranos.pages.dev/ctf ·
> Source: https://github.com/kenlacroix/ZiranOS
>
> Happy to go deep on the qemu-wasm bring-up, the ring-3 excursion, or the CTF
> design in the comments.

**Timing:** weekday (Tue–Thu) morning US Eastern. Lead the top comment with the one
war story you most want to discuss (e.g., compiling QEMU to WASM surfaced two latent
boot bugs). Don't post the Tier-2 solution even if asked — "that's the challenge 🙂".

---

## r/rust

**Title:** `A from-scratch x86-64 OS in no_std Rust that boots in the browser — with a CTF against its own kernel`

**Body:**
> Sharing a hobby project: **Ziran OS**, a bare-metal x86-64 OS in `no_std` Rust
> (`x86_64-unknown-none`) + hand-written Multiboot2/long-mode asm. It goes from the
> boot handoff through paging, a `#[global_allocator]` heap, a preemptive scheduler
> with hand-written context switching, a shell, a read-only filesystem, and ring-3
> userspace with an `int 0x80` syscall gate.
>
> The Rust-specific bits I think this sub will like:
> - The `unsafe`/asm discipline: every block documents *why* it's sound (MMIO is
>   volatile, the fabricated `iretq` frame, the syscall ABI), and it gets reviewed.
> - The filesystem is deliberately total and panic-free over hostile input —
>   checked arithmetic everywhere, ten classes of corrupt image rejected with a
>   specific error, no `unsafe` in the parser. That rigor is the whole security
>   lesson: a bounds check is only as good as the length it trusts.
> - You can **boot the real kernel in your browser** (QEMU→WASM) and there's a CTF
>   where you exploit the kernel's own filesystem — Tier 1 in-browser, Tier 2 a real
>   remote capture.
>
> Built openly with AI assistance, but CI *boots* the kernel on every push, so it's
> not vibes. Repo + build log: https://github.com/kenlacroix/ZiranOS · boot it:
> https://ziranos.pages.dev

---

## r/osdev

**Title:** `Ziran OS: from-scratch x86-64 kernel that boots in a browser via QEMU-WASM (+ a self-CTF)`

**Body:**
> Another hobby OS, but two angles this crowd might not have seen:
>
> - **QEMU compiled to WebAssembly boots the real ISO in a browser tab** — long
>   mode, VGA, PIC/PIT, PS/2, the works — to an interactive serial shell. Not v86
>   (32-bit, can't do long mode), not a replay. Bring-up surfaced two real boot bugs
>   (an unloaded GOT on the flat `-kernel` path, SSE not explicitly enabled).
> - **A CTF against the kernel's own boundaries.** The filesystem mount is the trust
>   boundary; the challenge is to craft an image that makes it read past where it
>   should. Tier 1 is white-box in the browser; Tier 2 is a remote capture where the
>   secret lives only in the server instance's RAM (per-session, via QEMU `fw_cfg`).
>
> Hand-written Multiboot2 header + the 32→long-mode transition in asm; own page
> tables (survives the CR3 switch); frame + heap allocators; round-robin then
> preemptive scheduler; ring 3 via a fabricated `iretq`; syscalls through a DPL-3
> `int 0x80` gate. Concept docs per subsystem, from first principles.
>
> Boot it: https://ziranos.pages.dev · source: https://github.com/kenlacroix/ZiranOS

---

## lobste.rs

**Title:** `Ziran OS: a from-scratch Rust kernel that boots in your browser, with a CTF against itself`
**Tags:** `osdev` `rust` `virtualization` `security`
(Link to the repo; add an authored-by "This is my project" note. Lead the text box
with the browser-boot + CTF hook, same as the r/osdev body, trimmed.)

---

## This Week in Rust (submission)

Submit the best single blog post (the qemu-wasm bring-up or the CTF/security post)
to the "Call for participation / blog posts" via the TWiR repo PR or form.

> **Ziran OS — booting a from-scratch Rust kernel in the browser, and building a
> CTF against it.** A `no_std` x86-64 hobby OS: the post covers [compiling QEMU to
> WebAssembly to boot the real 64-bit kernel in a tab / the filesystem trust
> boundary and how a length-confusion bug becomes a capture-the-flag]. Live demo +
> source linked.

---

## LinkedIn (portfolio / professional)

> I built an operating system from scratch — and you can boot it in your browser.
>
> **Ziran OS** is a hobby x86-64 OS written from the ground up in Rust and assembly:
> bootloader, virtual memory, a scheduler, a filesystem, user-mode processes with
> system calls. No frameworks, no OS underneath — just the manual and the hardware.
>
> Two things I'm proud of:
>
> → **It's not a screenshot — it's live.** Real QEMU compiled to WebAssembly boots
> the actual 64-bit kernel in a browser tab, next to a guided tour that explains each
> layer as it runs.
>
> → **I turned it into a security challenge against my own kernel** — a two-tier CTF,
> including a genuine *remote* capture running on hardened infrastructure I built and
> operate.
>
> It's also an experiment in **rigorous AI-assisted engineering**: built with an AI
> coding agent, but with the discipline that makes that trustworthy — CI that boots
> the kernel on every commit, fuzzed trust boundaries, and staff-level review of
> every unsafe line. The honest build log — including what broke — is public.
>
> For anyone hiring or curious about systems work, low-level Rust, or how to actually
> get quality out of AI-assisted development: boot it, read it, break it.
>
> 🔗 Boot it in your browser: ziranos.pages.dev
> 🔗 Source + build log: github.com/kenlacroix/ZiranOS
>
> #Rust #OperatingSystems #SystemsProgramming #AI #SoftwareEngineering

---

## X / Twitter (thread)

1/ I wrote an operating system from scratch in Rust — and you can boot the real
64-bit kernel in your browser right now. Then steal a flag from it.
🔗 ziranos.pages.dev

2/ It's the whole stack, hand-built: Multiboot2 + long-mode asm, paging, a heap, a
preemptive scheduler, a shell, a filesystem, ring-3 userspace with syscalls. QEMU-only
by design.

3/ The demo isn't a video. It's real QEMU compiled to WebAssembly running the actual
kernel to an interactive shell in a tab — with a guided tour that explains each layer
as it boots.

4/ Then there's a CTF against the kernel itself. Tier 1: craft a disk image in your
browser and make the filesystem leak a hidden flag. Tier 2: a *remote* capture — the
flag lives only in a server's RAM. Take it over the wire.

5/ Built openly with an AI coding agent — but CI *boots* the kernel on every push, the
filesystem is fuzzed, and every unsafe line is reviewed. The build log (what broke
included) is public. Not vibes.

6/ Boot it · read it · break it:
ziranos.pages.dev · github.com/kenlacroix/ZiranOS

---

## Elevator pitch (bio / one-liner)

> A from-scratch x86-64 OS in Rust that boots in your browser — with a two-tier CTF
> against its own kernel. Built openly with AI, verified like it wasn't.

---

## Reusable talking points (for comments / interviews)

- **Why browser-boot matters:** most hobby OSes are screenshots; almost none are one
  click to a live kernel. It's the difference between *looking at* the project and
  *using* it.
- **Why the CTF matters:** the security track attacks the kernel's *own* boundaries
  to learn what they enforce — not to harden against the world (an explicit non-goal).
  Tier 2 makes it a real remote capture: the secret is never in anything you're given.
- **The AI-workflow story:** the differentiator isn't "AI wrote it," it's the
  verification harness that makes AI-assisted systems work trustworthy — CI boots the
  kernel, boundaries are fuzzed, unsafe is reviewed, and the retros are honest.
- **On scope:** the biggest risk in a hobby OS is scope creep toward "real" features.
  Ziran stays QEMU-only, no-POSIX, no-hardening — small, finished, and demoable beats
  sprawling and 80% done.
- **Do NOT** post the Tier-2 exploit, even when asked. "It's the challenge — writeups
  welcome as GitHub issues once you've got it."
