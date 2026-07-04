# Browser + remote CTF — eng-plan (M16 delivery, two tiers)

Locks the Think→Plan for turning M16 ("break the filesystem boundary", PLAN §8)
into a **playable capture-the-flag** with two delivery tiers:

- **Tier 1 — in-browser white-box.** The live qemu-wasm boot (STATUS "Teaching
  tool" row) gains a shell path to submit a crafted filesystem image and watch
  the deliberately-loose extent check leak bytes it shouldn't. Zero setup, runs
  entirely in the visitor's tab. This is the teaching artifact.
- **Tier 2 — remote capture.** A server-hosted kernel instance where the flag
  lives **only in that process's RAM**, reachable only by landing the exploit
  over the wire. This is the part that is genuinely uncheatable and genuinely
  rare — a from-scratch hobby kernel as a remote pwn target, with a browser
  sandbox to develop against.

The security *lesson* is M16's: **a bounds check is only as good as the length it
trusts.** The delivery work is what makes that lesson something a stranger can
click into and feel, rather than read about.

This plan sits downstream of the M16 seed already planted (STATUS M12: a hidden
`FLAG{…}` with no directory entry) and the qemu-wasm live boot (STATUS: "verified
booting to an interactive `ziran:/>` shell in a browser"). It does **not** cover
M15 (privilege boundary) — that is its own track.

---

## 0. The one load-bearing decision (read this first)

Tier 1 and Tier 2 are **not the same challenge at two scales.** They rest on two
different vulnerability classes, and conflating them is the trap:

- **Tier 1 is in-image aliasing (white-box).** The player supplies the image
  *and* it contains the flag bytes. They craft a directory entry whose extent
  `(offset, length)` overlaps the unlisted flag region — bytes that are within
  `total_size` (so they pass "every-extent-in-bounds") but have no directory
  entry (so `ls`/`cat` can't normally reach them). `cat`-ing that file dumps the
  flag. This *demonstrates* the primitive but is **not a secret capture** — the
  player already holds every byte. Dumping the wasm linear memory, or running
  `strings` on the kernel image, reaches the flag just as fast. That is fine and
  must be stated plainly: Tier 1 is a *sandbox to learn the bug in*, not a vault.

- **Tier 2 must put the secret outside the player's bytes.** For a real capture,
  the flag can never be in anything the player receives — it lives only in the
  **server** process's memory. That forces a *stronger* bug than in-image
  aliasing: the mount validator must trust an **attacker-controlled length**
  (the image header's `total_size`) over the **real backing-buffer length**, so
  a crafted extent reads past the player's uploaded bytes into an adjacent
  server-side allocation where the flag was placed. The player uploads a small
  image, declares a large `total_size`, and points an extent into the flag.

This distinction is the whole plan. Tier 1 reuses the M16 seed as-is. Tier 2
requires deliberately engineering a heap-adjacent, out-of-buffer read — a
*different, more powerful* vulnerability — and doing so in a way that is
deterministic and reachable (see §3.4 honest unknowns; naive Rust slicing
*panics* rather than reading OOB, so the flag and the mounted image must share
one real backing allocation).

If Tier 2's cost (see §5) isn't worth it yet, **Tier 1 alone is a complete,
shippable deliverable** — build it, launch it, and gate Tier 2 on traction.

---

## 1. Office hours (forcing questions)

**Q1 — The single thing a visitor understands after this that they didn't.**
That an off-by-one in a bounds check is not a theoretical defect — it is an
exfiltration primitive. They will have *personally* made a filesystem hand them
bytes it was built to hide, by lying about a length in a header. And in Tier 2,
that the same trick works against a machine whose memory they cannot see.

**Q2 — "Done" as an observable.**
- Tier 1: on the live qemu-wasm page, a visitor pastes/loads a crafted image via
  a new shell command, runs `cat` on a crafted file, and sees the planted
  `FLAG{…}` print — bytes that `ls` never listed. A spoiler-gated walkthrough
  explains why.
- Tier 2: a visitor connects to the hosted instance (browser terminal **or**
  `nc host port`), develops their exploit against the local sandbox, submits the
  crafted image to the remote, and captures a flag that was **never in any bytes
  they were given** — provably, because the same image against their own local
  sandbox yields a *different* (or absent) flag.

**Q3 — The smallest version that still teaches it.**
- Tier 1 minimal: the existing live boot + one new shell command (`load <b64>`
  or a paste-mode) that mounts an image from user-supplied bytes, + the existing
  M16 seed + one static walkthrough page. No accounts, no scoring, no upload UI
  beyond a textarea/paste.
- Tier 2 minimal: **one** server instance, a WebSocket↔serial bridge, the flag
  planted in that process's RAM, and per-connection ephemeral QEMU. No
  leaderboard, no auth, no multi-challenge platform.

**Q4 — Working vs. accidentally working.** The trap in Tier 2 is a "capture"
that is really Tier 1 in disguise — the flag ended up in a byte range the player
controlled, so they didn't exploit anything, they just read their own upload. The
**gate** is a differential test: the *same* crafted image must capture the
*server's* flag but not reveal it against a *local* sandbox seeded with a
different flag. If it does, the read genuinely crossed out of the player's buffer
into server memory. "It printed `FLAG{…}`" is not enough; "it printed the
*server's* flag, which was never in my image" is the proof.

**Q5 — What could stall this for days.**
- Tier 1: making a raw byte image loadable over the serial console without a file
  upload path — encoding (base64 over a line-oriented console), size limits, and
  the fact that a *bad* image must be *rejected cleanly* (the existing 10
  corrupt-image rejections must still hold; the loose check is a specific,
  intended gap, not "parsing got sloppy").
- Tier 2: (a) the Rust-panic wrinkle — engineering an OOB read that actually
  *reads adjacent memory* instead of panicking on a slice bound; (b) the ops
  reality of hosting a service you invite strangers to attack (see §4); (c) the
  ephemeral-spawn lifecycle (reap hung QEMUs, cap concurrency) before it DoSes
  the host.

**Q6 — Non-goal drift.** The sharp one. Tier 2 introduces a **network service** —
and PLAN §1 lists "no networking (beyond the stretch stub)". The defense is that
the network lives entirely in *delivery/challenge infrastructure*, never in the
kernel: the kernel gains nothing networked, speaks only over its existing serial
console, and does not know it is being served remotely. See §2 for the explicit
ruling — this needs a line in PLAN.md before Tier 2 is built, not a silent slide.

---

## 2. Scope-guard verdict

**Verdict: Proceed on Tier 1; Hold-with-explicit-decision on Tier 2.**

**Tier 1 is squarely in scope.** It is the *delivery* of M16, which is a named
security milestone (PLAN §8). It adds one shell command and one web page; the
"vulnerability" is the M16 seed that already exists by design. Nothing here
touches a non-goal. Build it.

**Tier 2 requires an explicit PLAN.md decision** because it stands up a network
service, and networking is a §1 non-goal. The argument for allowing it, to be
recorded in PLAN §8 (not assumed):

- The non-goal is about **the kernel not growing a network stack** — no sockets,
  no NIC driver, no TCP/IP in ring 0. Tier 2 honors that completely: the kernel
  emits and reads bytes on COM1 exactly as it does under `make console`. The
  WebSocket/TCP lives in a **host-side bridge process** that pipes serial ↔
  socket. The kernel is unaware.
- It is challenge infrastructure, in the same category as the qemu-wasm hosting
  (STATUS/`docs/DEPLOY.md`) already accepted as delivery, not kernel scope.

**Defer / keep out, explicitly:**
- **Any in-kernel networking** — no NIC, no stack, no `SYS_socket`. If Tier 2
  ever tempts "just let the kernel speak the protocol," stop; that is the §1
  line and this plan does not cross it.
- **A CTF *platform*** — no accounts, no scoring, no multi-challenge framework,
  no database of solves. One flag, one target, optional writeup-as-GitHub-issue.
- **Persistence** — instances are ephemeral and stateless; nothing a player does
  survives their disconnect. No saved progress, no user data (which also removes
  a whole class of privacy/security obligation).
- **Making the *whole* filesystem insecure.** The Tier-2 bug is one deliberate,
  documented, contained gap (the trusted-length read). The other ten corrupt-
  image rejections and the panic-free guarantees stay intact. This is
  adversarial *learning* per `/red-team`, not de-hardening the parser.

If Tier 2's infra starts growing toward "a hosting product," re-open this section.

---

## 3. Eng-plan

### 3.1 Shared groundwork: the image-load channel

Both tiers need one thing the kernel can't do today: **accept a filesystem image
from outside the boot mkfs.** Today `boot_image()` builds the disk in-kernel
(STATUS M11); there is no way for an interactive user to supply bytes. Add:

- A shell command — `load` (or `mount <name>`) — that reads a **base64-encoded
  image** from the serial console into a heap `Vec<u8>`, decodes it, and calls
  `Fs::mount` on the result, exposing it under a mountpoint (e.g. `/mnt`). Serial
  is line-oriented, so accept the blob as one long line (or a `begin`/`end`
  fenced paste), with a hard **size cap** (e.g. 64 KiB) checked *before*
  allocation.
- `Fs::mount` is unchanged for Tier 1 — it already re-derives the directory from
  raw bytes and validates magic/version/total_size/dir-fits/extents (STATUS M11).
  The M16-seed looseness (extents bounded by `total_size`, not by per-file
  disjointness) is exactly the intended gap; the load path just *reaches* it.
- Decoding must be **total and panic-free** — a bad base64 char or an
  oversize/truncated blob returns an error the shell prints, never a panic. This
  is the same discipline as the existing mount trust boundary; the input is now
  live-hostile, so it matters more.

This one command is the input vector for Tier 1 (paste in the browser textarea)
*and* Tier 2 (the remote submits the same way over the bridge).

### 3.2 Tier 1 — in-browser white-box challenge

**Kernel side:** the `load` command above + the existing M16 seed. The exploit is
authored *by the player*: they build an image whose directory lists a benign file
but whose extent `(offset, length)` covers the unlisted flag bytes. `cat`
zero-copies that extent out. No kernel change beyond `load`.

**Web side:** a challenge page under `web/` (sibling to `qemu-wasm.html`):
1. The live boot (reuse the verified qemu-wasm wiring, STATUS).
2. Challenge framing: "A flag is hidden on this disk with no directory entry.
   `ls` will never show it. Read it anyway." A textarea to paste a crafted image
   (base64), a "load" button that types `load <b64>` into the serial console.
3. A **spoiler-gated walkthrough** (predict→observe→explain, matching the tour's
   existing pedagogy): the image format, where the loose check is, how to craft
   the aliasing extent, and — stated honestly — *why this one is a sandbox*
   (the flag is in your tab's memory; the point is the primitive, not secrecy).
4. Optional: a tiny in-page image builder (form → base64) so players who don't
   want to hand-assemble bytes can still play. Keep it out of the minimal cut.

**Honesty requirement:** the page must say, up front, that Tier 1 is white-box —
memory-dumping DevTools or `strings` on the wasm reaches the flag trivially, and
that is *expected*. Pretending otherwise is the one thing that would make it read
as a gimmick. Point players who want a real capture at Tier 2.

### 3.3 Tier 2 — remote capture

**The vulnerability (the design work):** escalate from in-image aliasing to a
**trusted-length out-of-buffer read.** The server:
1. Allocates the flag bytes into the kernel heap.
2. Accepts the player's uploaded image via `load` into a heap buffer.
3. The mount validator's *intended flaw*: it bounds extents against the image
   header's self-declared `total_size` (attacker-controlled) rather than the
   **actual decoded byte-length** of the uploaded buffer. A crafted extent with
   `offset+length` inside the *claimed* `total_size` but beyond the *real* buffer
   reads adjacent heap — where the flag was placed.

The teachable bug: *you validated against a length from the data, not the length
of the memory you actually have.* This is a real, common class (CVE-shaped), and
it is distinct from — and strictly stronger than — the Tier-1 in-image alias.

**The Rust wrinkle (honest unknown, §3.4):** a zero-copy `&buf[a..b]` with `b`
past `buf.len()` **panics**, it does not read OOB. So the read must land inside a
*real* allocation. The likely shape: the server builds one large backing buffer
`[uploaded image bytes | padding | flag bytes]` and hands `mount` a *view/length*
that the loose check fails to enforce, so extents range over the whole real
buffer including the flag tail. Nailing the exact layout (single `Vec`, computed
flag offset, deterministic adjacency, no ASLR to fight since it's our allocator)
is build-phase work — see §3.4.

**The bridge + spawner (host-side, no kernel change):**
```
player (browser xterm  or  nc host port)
   │  WebSocket  (browser)      │  raw TCP  (terminal)
   ▼                            ▼
bridge process (host-side, NOT in the kernel)
   • per connection: spawn a fresh QEMU booting the kernel,
     flag planted into THAT instance's RAM at spawn time
   • pipe kernel COM1  ⇄  socket   (the make-console serial path, over a socket)
   • enforce: per-session timeout, concurrency cap, per-IP rate limit,
     cgroup mem/CPU cap per QEMU, reap on disconnect
   ▼
QEMU (per-connection, ephemeral)  →  ziran kernel, serial only
```
- Reuse the serial console end-to-end (STATUS "Serial console" row): the kernel
  already does non-blocking `recv`/`read_byte`; the bridge just moves those bytes
  over a socket instead of a terminal.
- **Ephemeral per connection is mandatory**, not an optimization: players crash
  and corrupt the kernel constantly; one session must not affect another, and a
  hung QEMU must be reaped, or a handful of connections DoS the host.
- The bridge **never shells out with player input.** Spawn QEMU with an argv
  array; plant the flag by writing a file the QEMU reads, never by interpolating
  player bytes into a command line. (See §4 — this is a likelier hole than any
  QEMU 0-day.)

### 3.4 Edge cases & honest unknowns

- **[T2] The panic-vs-read wrinkle** (headline unknown). The OOB read must stay
  within a real backing allocation or Rust panics instead of leaking. Resolve in
  the build by fixing the allocation layout: flag and image in one buffer, loose
  check gates the sub-range. Confirm the leak is deterministic across boots.
- **[T2] Differential-capture proof** (Q4 gate). Must demonstrate the *same*
  crafted image captures the server flag but not a locally-seeded different flag —
  otherwise it's Tier 1 in disguise. Bake this into the verification, §3.5.
- **[both] `load` must stay panic-free on hostile input.** Bad base64,
  oversize, truncated, lying header fields → clean `FsError`, never a panic. The
  existing 10 corrupt-image rejections (STATUS M12) must still pass unchanged;
  the loose check is *one specific* gap, not general looseness.
- **[both] Size caps before allocation.** Check the declared/encoded size against
  a hard cap *before* allocating, or a huge `total_size` becomes a memory-
  exhaustion DoS (in Tier 1 it hangs the visitor's tab; in Tier 2 it hits the
  server — cgroup caps are the backstop, but reject early).
- **[T2] Instance lifecycle.** Timeout, reap-on-disconnect, concurrency cap. An
  un-reaped QEMU per abandoned connection is the most likely way the host falls
  over — more likely than any exploit.
- **[T1] Serial paste ergonomics.** A multi-KB base64 blob over a line-oriented
  console may need chunking or a fenced `begin/end` paste mode; xterm-pty paste
  behavior in the browser needs a look.

### 3.5 Verification plan (working vs. accidentally working)

- **Tier 1, deterministic (CI-able):** a boot self-test crafts an aliasing image
  in-kernel, `load`s it, and asserts `cat` returns the exact planted flag bytes —
  proving the loose extent check leaks the unlisted region. Also assert the ten
  corrupt-image rejections still hold (the looseness didn't regress the trust
  boundary). This pins the *primitive* without a browser.
- **Tier 1, manual (browser):** load the crafted image via the page, see the flag
  print. One-time eyes-on check, like the other web verifications.
- **Tier 2, the capture gate (Q4):** the differential test — script a client that
  submits the crafted image to (a) the hosted instance (flag `A`) and (b) a local
  sandbox seeded with flag `B`; assert it recovers `A` from the server and does
  **not** recover `A` from the local sandbox. This proves the read crossed out of
  the player's bytes into server memory. This is the test that separates a real
  capture from reading your own upload.
- **Tier 2, ops:** load-test the bridge — N concurrent connections spawn and reap
  cleanly, a hung guest is killed on timeout, the concurrency cap holds, and a
  flood is rate-limited before it exhausts the host.

---

## 4. Tier-2 deployment & isolation (the "can they reach my LAN?" answer)

The threat model is unusual: **the player has full control inside the guest** —
running arbitrary code as ring 0 in the guest kernel *is the challenge*. So no
in-guest defense matters; security is everything *outside* QEMU. Design as if the
guest is 100% hostile, because it is.

**What does and does not provide isolation:**
- **Cloudflare Tunnel is ingress control, NOT a breakout boundary.** `cloudflared`
  makes an *outbound* connection to Cloudflare, so there are zero inbound open
  ports and the home IP stays hidden — that solves "how do people reach it without
  port-forwarding." It does **nothing** to contain a breakout; an escaped attacker
  pivots laterally on the LAN, not back through the tunnel. Different problem.
- **Do NOT use a privileged LXC (the Proxmox gotcha).** Proxmox LXCs are often
  privileged by default, and a privileged LXC is explicitly not a security
  boundary — a root escape in it is root on the host. For a deliberately-hostile
  workload, use a proper **VM (KVM)**, and run the per-connection QEMU *inside*
  that VM. A guest escape then lands in a throwaway inner VM; reaching the host
  needs a *second, different* hypervisor escape.
- **Harden QEMU** (shrink the surface the guest can attack):
  `-nodefaults`, serial only, **no emulated NIC** (the kernel has no networking —
  a non-goal that conveniently deletes a whole device-bug class), no USB, no VGA;
  `-sandbox on` (seccomp); run QEMU as an unprivileged user, no capabilities;
  keep QEMU patched.
- **Network isolation is the real safety net.** Put the VM on its own VLAN with a
  firewall that **denies all traffic to the LAN** (no other homelab hosts, no
  NAS) and permits only the outbound Cloudflare connection. Then even a chained
  QEMU-escape *and* VM-escape lands on a disposable island that can see nothing
  else. Make the reachable blast radius equal to nothing.

**The risks people underrate (likelier than any 0-day):**
- **The bridge's own code.** Interpolating player bytes into a QEMU command line
  or the flag-planting step is command injection that needs no escape at all.
  argv arrays, file-based flag planting, validated/bounded uploads.
- **Resource-exhaustion DoS.** Not a breakout but a "mess with my host" in
  practice. Cap concurrent instances, per-IP rate-limit (Cloudflare helps),
  cgroup CPU/mem per QEMU, hard per-session timeout with auto-reap.

**Bottom line & recommendation.** With VM-not-LXC + stripped/seccomp'd QEMU + a
VLAN island + a bridge that never shells out, the residual risk is a sophisticated
attacker chaining two distinct hypervisor escapes to reach a dead-end box — not a
realistic threat for a hobby CTF. The likelier problems (bridge bugs, DoS) are
fully in our control.

**Cheapest way to erase the worry: host Tier 2 on a disposable $5/mo cloud VPS,
not the homelab.** Then "break out and reach my LAN" isn't a question — there is
no LAN behind it, just a firewalled throwaway to nuke and rebuild. If this might
hit Hacker News, run Tier 2 on a VPS and keep Proxmox for build/dev. The homelab
path is viable with the hardening above, but the VPS removes an entire category of
reasoning for the price of a coffee.

---

## 5. Build order (each a small, testable step)

**Phase A — Tier 1 (ship this first; complete on its own):**
1. **`load` shell command + base64 decode**, panic-free, size-capped, mounting an
   uploaded image at `/mnt`. Self-test: load a known-good image, `ls`/`cat` it;
   load malformed images, assert clean rejection (the ten still hold).
2. **The white-box challenge page** under `web/`: live boot + paste/load UI +
   spoiler-gated walkthrough, honest "this is a sandbox" framing. Manual browser
   verify: craft an aliasing image, capture the seeded flag in-tab.
3. **CI self-test** asserting the in-image alias leaks the planted flag and that
   the corrupt-image rejections are intact. Marker bump.
4. Docs: fold into the M16 blog post / `docs/concepts/filesystem.md`; note the
   loose-check gap explicitly. `/red-team` applies here — the FS boundary now has
   a live attacker path.

**Phase B — Tier 2 (gate on: Phase A shipped + a decision it's worth the ops):**
5. **The trusted-length OOB read.** Engineer the flag-adjacent allocation and the
   loose length check; prove the leak is deterministic and doesn't panic (the
   §3.4 unknown). Differential self-test (server flag vs local flag).
6. **The bridge + ephemeral spawner** (host-side, no kernel change): serial↔socket
   over WebSocket + raw TCP, per-connection QEMU, timeout/reap/concurrency/rate
   limits. Load-test it.
7. **Deployment**: VPS (recommended) or hardened Proxmox VM per §4 — stripped
   QEMU, VLAN island, Cloudflare Tunnel, argv-only spawner. Smoke-test the
   capture end-to-end from a clean client.
8. **Publish**: the remote target's address + a "develop locally, capture
   remotely" writeup; accept solver writeups as GitHub issues (repo activity →
   trending). PLAN.md updated to record the Tier-2 network-service decision (§2).

---

## 6. Relationship to the roadmap

- **M16** is the security content; this plan is its *delivery*. Tier 1 is the
  minimal M16 done publicly; Tier 2 is the ambitious version.
- **M15** (privilege boundary) is independent — a second, different CTF surface
  (ring-3 escape) that could get the same two-tier treatment later. Out of scope
  here; note it as the natural follow-on.
- **PLAN §1 non-goal (networking):** Tier 2 requires the explicit §2 decision
  recorded in PLAN §8 before build — the kernel stays networkless; the network is
  host-side delivery infra only.
