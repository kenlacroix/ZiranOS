# Hello over a socket that isn't

*Milestone 14 — the networking stretch, and the last item on the roadmap. A
loopback interface, a socket you can bind and send from, "hello" delivered by
address — and no IP, no protocols, no wire, no peer. The milestone where the
hardest work was deciding what **not** to build.*

> New to this? Read [docs/concepts/networking.md](../concepts/networking.md)
> alongside — it builds the socket abstraction from first principles (a named
> endpoint with a receive queue, reached by address) and explains why loopback is
> the one interface where "the network" is provably just a buffer.

Networking is a **non-goal** for this project. It says so in the first paragraph of
the plan: no networking stack, beyond a stretch-goal stub. So this milestone is a
strange one to write up, because the honest version of "what I built" is mostly a
list of what I *didn't*: no TCP, no IP, no ports, no checksums, no ARP, no device
driver, no packets on any wire. QEMU boots with no network at all.

What's left after all those subtractions is one small, real thing: a **socket**.

> **The lesson, up front: a socket is a named endpoint with a receive queue,
> reached by address, decoupled from whoever's on the other end — three properties
> a function call collapses and a network keeps apart.**

And the meta-lesson, which is the actual story of this milestone: **on a project
whose biggest risk is scope creep, deciding the scope *is* the milestone.** The
code took an afternoon. The scope-guard call — loopback, not virtio-net; a stub,
not a stack — is the part worth the post.

### Milestone 14 — networking stub — checklist

```
Think
- [x] /milestone-office-hours — six forcing questions answered in the eng-plan
- [x] /scope-guard — verdict: Hold (loopback-only); virtio-net deferred as its
      own future device-driver milestone; IP/TCP/ports/wire banned

Plan
- [x] /eng-plan — approach, machine-state preconditions, edge cases, verification

Build
- [x] Working, demoable state: "hello" delivered by address over loopback
- [x] `make` clean: assembles, compiles, links; no unsafe
- [x] No new compiler warnings

Debug
- [x] No debugging story — no hardware surface to fault (that's the point of
      choosing loopback over virtio-net)

Review
- [x] /kernel-review — clean stub; no critical/high; two low doc-notes applied

Security
- [x] /red-team — skipped: a loopback stub adds no privilege/syscall/FS attack
      surface (no untrusted input crosses a boundary; addresses are kernel-chosen)

Reflect
- [x] /retro — the honest post-mortem (below)
- [x] /document-milestone — STATUS.md ✅, README, concept doc, this post, web tour
```

## Office hours: what a socket actually is

The forcing question that mattered was Q1 — *what will I understand after this that
I don't now?* Not "how does a network work" (out of scope, forever). The sharp
answer: **what a socket is underneath the abstraction.**

When one part of a program hands data to another, it calls a function — it holds a
reference to the callee, and control returns when the callee is done. A network
endpoint has none of that. You send to an *address*, not an object you hold. The
sender doesn't run the receiver; the bytes wait in a queue. And a *device* sits in
the middle — you hand bytes to an interface, and the interface delivers them, as a
separate step. A socket is the handle that gives you those three properties.

So "done" isn't "bytes came back." That's an echo, and an echo teaches nothing
about addressing. Done is a *contrast* asserted in one boot: a frame sent from
address 1 to address 2 is `None` at the receiver **until the device polls** (the
layers are separate); it then arrives byte-exact with `src`/`dst` preserved; a
socket bound to a *different* address gets nothing (delivery is by address, not
broadcast); and a frame to an unbound address drops cleanly. Those four are what
make it addressed delivery instead of a buffer with extra steps.

## Scope-guard: the milestone inside the milestone

This is the one milestone whose whole risk is scope, so scope-guard wasn't a
rubber stamp — it was the design. The plan offered two ways to do "networking
stub": a loopback, or a "trivial" virtio-net driver. The instinct pulling toward
virtio was strong, and worth naming honestly, because it's *exactly* the scope-creep
gravity the project exists to resist: **a real driver feels more legitimate than a
loopback.**

It isn't. Here's the reasoning that settled it:

- A **loopback** teaches the socket/interface/addressing lesson — the whole payoff
  — with zero new hardware, zero protocol, and no QEMU changes. It slots into the
  existing `init()/self_test()/marker` pattern.
- **virtio-net** would pull in PCI enumeration, virtqueues, DMA rings, the QEMU
  `-netdev` plumbing, and — to say "hello" to anyone — a peer, which means either a
  protocol stack (the non-goal) or another loopback anyway. Its *networking* payoff
  still bottoms out at "move a frame."

So the verdict was **Hold, loopback-only** — and, crucially, the deferral was
written down as its own thing: virtqueues *are* worth a future milestone, but as
**device-driver learning**, framed that way, not smuggled in under "networking
stub." The reframe that made the call easy: **"trivial" isn't "lesser" — it's
"scoped."** The loopback isn't the cheap version of the real thing; it's the
correct version of *this* lesson.

## Eng-plan and what got built

`src/net.rs` is one loopback interface behind one lock: a `Frame { src, dst,
payload }`, a global `Net` holding the device's transmit queue and a per-address
receive queue, and a `Socket(u16)` handle with `bind`/`send`/`recv`/`close`. The
one deliberate design choice is the **two queues**: `send` only appends to the
device's TX queue; a separate `net::poll()` drains it and routes each frame to the
socket bound to its `dst`. That models a real NIC's TX/RX split — and it's what
makes `recv`-before-`poll` return `None`, the single sharpest proof that the socket
layer and the device layer are genuinely separate.

The self-test says it best, straight off the serial log:

```
[ok] net: loopback delivered "hello from ziran" from addr 1 to addr 2 byte-exact;
     wrong-address and unbound-destination frames did not arrive (1 dropped, no-route)
M14: loopback online
```

## Retro: the honest part

There is no debugging war story, and that's the honest headline — because there's
no hardware surface to fault. No asm, no paging, no IRQ path, no device registers.
The only thing that "broke" was a dead-code warning on an accessor I didn't end up
using, deleted in seconds. A milestone with no bug to hunt is unusual for this
project, and the reason is precisely the scope call: loopback has nothing that can
silently triple-fault.

So the honest lede isn't a bug — it's the instinct I had to talk myself out of. I
nearly built virtio-net because *realer felt better*, and that instinct is the exact
gravity that turns a hobby OS into an unfinished protocol stack. The thing worth
keeping: for a milestone whose risk is scope rather than mechanics, spend the effort
on the **boundary** — what you refused to build, and whether the refusal is written
down durably so a future you doesn't re-litigate it. That's why the deferral (virtio
as its own milestone) lives in the concept doc and STATUS, not just a commit message.

Kernel-review confirmed the small stuff (the plain `spin::Mutex` is sound because
`NET` is never touched from an interrupt and is always locked with interrupts
enabled, so a preempted lock-holder still makes progress; no `unsafe`; no leaks) and
flagged two future-caller footguns — a dropped-but-unclosed `Socket` leaks its
binding, and queue depth is unbounded — both now documented rather than "fixed,"
because the honest fix (a `Drop` that locks the global) would introduce a
reentrant-deadlock hazard worse than the leak.

## The roadmap is complete

M14 was the last item, and it was built last on purpose — after the security track
(13, 15, 16), because breaking a real boundary taught more than a stub, and a stub
was never going to be the interesting part. So the finished shape of the project is:
the core arc from bare metal to a file manager (M0–M12), the first privilege
boundary and its flag capture (M13, M15), the filesystem boundary and its flag
capture (M16), and this — a socket you can send "hello" over, delivered by address,
with nothing underneath but a buffer.

The whole project was an argument that you understand a thing by building it from
first principles and refusing the parts that would turn understanding into
open-ended engineering. This milestone is that argument in miniature: the smallest
honest socket, and a firewall around it. The wire was never the lesson.
