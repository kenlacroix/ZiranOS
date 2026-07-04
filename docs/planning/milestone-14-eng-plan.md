# Milestone 14 — Networking stub (loopback): eng-plan

*Think phase done: `milestone-office-hours` (six forcing questions, below) +
`scope-guard` (**HOLD**, loopback-only; virtio-net deferred as its own future
device-driver milestone, not smuggled in here). `/red-team` **skipped** — a
loopback stub adds no privilege/syscall/filesystem attack surface (no untrusted
input crosses a boundary; addresses are kernel-chosen `u16`s).*

## The one-sentence learning goal (office-hours Q1)

What a socket actually *is* underneath the abstraction: a named endpoint with a
receive queue, decoupled from whoever is on the other end, reached by
**addressing** rather than by a direct call — so that "send to an address," "a
device moves the bytes," and "the endpoint bound to that address receives them"
are three separable layers, none of which needs a wire, a protocol, or a peer
that knows you.

## Done, as observable serial behavior (Q2/Q4)

One boot, asserted in `net::self_test()`, then a `M14:` marker CI greps:

- Bind addr `2` (receiver) and addr `1` (sender). `sock1.send(to: 2, "hello from
  ziran")`. **Before `poll()`, `sock2.recv()` is `None`** — the socket call only
  queued the frame on the device; nothing has been delivered yet.
- `net::poll()` (the loopback device step) → `sock2.recv()` returns the frame,
  `payload` **byte-equal** to `"hello from ziran"`, `frame.src == 1`, `dst == 2`.
- A socket bound to addr `3` sees `recv() == None` — delivery is **by address**,
  not a broadcast/echo (the anti-"it just echoed to everyone" guard).
- `sock1.send(to: 9, …)` with `9` unbound → the frame is **dropped cleanly** at
  `poll()` (a `NoRoute` count), no panic; every other queue is unaffected.
- `recv()` on a drained queue → `None` (would-block; the queue is real, not a
  latched value). Re-`bind(1)` → `AddrInUse`. Oversized payload → `TooLong`.

Plus a `net` shell built-in that runs a live loopback round-trip and prints
`lo: 1 -> 2 "hello…" (N bytes)`.

## Minimal cut (Q3)

One loopback interface, a `Frame { src: u16, dst: u16, payload }`, a socket table
keyed by `u16` address, and `bind`/`send`/`poll`/`recv`. **No** IP, TCP, UDP,
ports, checksums, ARP, routing, real device, virtio, or QEMU `-netdev`. Addresses
are bare `u16`s. A single `MTU` bound on payload size is the *only* nod to real
framing (it teaches "a frame has a max size" and caps allocation — not a protocol).

**Explicit deferrals** (each a clean future milestone, not "while I'm here"): a
virtio-net driver (PCI enumeration + virtqueues — real device-driver learning, its
own milestone), any protocol header, ports, and a real off-box peer.

## Approach

- **New module `src/net.rs`.** Pure `no_std` + `alloc` Rust; no `unsafe`, no MMIO,
  no asm.
- **`Frame { src: u16, dst: u16, payload: Vec<u8> }`.** `PartialEq/Eq` for
  byte-exact assertions. `MTU` caps `payload.len()`.
- **The loopback device + socket table, one global** `static NET: Mutex<Net>`
  (`spin::Mutex`, the house style — cf. `FRAME_ALLOCATOR`, `SCHED`). `Net {
  tx: VecDeque<Frame>, sockets: BTreeMap<u16, VecDeque<Frame>>, dropped: u64 }`.
  - `tx` is the device's transmit queue; `sockets` maps a bound address to its RX
    queue. The **two queues are the lesson**: a socket call only touches `tx`; the
    device, when polled, moves frames from `tx` to the destination's RX — TX and RX
    are decoupled exactly as on a real NIC.
- **API** (a `Socket` is a thin handle — just its address; the queues live in the
  table, so no `Arc`/`RefCell`/lifetime machinery):
  - `Socket::bind(addr) -> Result<Socket, NetError>` — registers an empty RX queue;
    `AddrInUse` if taken.
  - `Socket::send(&self, to: u16, bytes: &[u8]) -> Result<(), NetError>` — `TooLong`
    if `> MTU`; else push `Frame { src: self.0, dst: to, payload }` onto `tx`.
  - `net::poll() -> usize` — drain `tx`; route each frame to `sockets[dst]` RX, or
    increment `dropped` if `dst` is unbound; return the count delivered. This *is*
    the loopback: TX becomes RX, routed by `dst`.
  - `Socket::recv(&self) -> Option<Frame>` — pop-front of `sockets[self.0]`; `None`
    if empty or unbound.
  - `Socket::close`/`net::unbind(addr)` — remove the queue, so the self-test leaves
    `NET` clean for the shell `net` command.
- **`NetError`**: `AddrInUse`, `TooLong`, plus a `poll` `dropped` counter for
  no-route (a drop is not an error to the sender — a real NIC doesn't know the peer
  is gone; the honest model is "sent, silently dropped," surfaced via the counter).

## Preconditions (machine state)

- **Heap initialized** — `Vec`/`VecDeque`/`BTreeMap` allocate; `net::self_test`
  runs in `kernel_main` *after* `heap::init` (and after `fs`/`shell` self-tests),
  before the scheduler starts. The `net` shell command runs later in task context.
- **Never entered from an interrupt handler.** All callers are task/self-test
  context, so a plain `spin::Mutex` suffices — no IF-guard, no IRQ re-entrancy (the
  concern that made `SCHED` interrupt-safe does not apply here). Stated so a future
  "deliver on a timer IRQ" change knows it must revisit the lock.
- Long mode, paging on (heap lives in the M8 window) — inherited, nothing new.

## Data flow

```
sock1.send(to=2, bytes)  -> NET.lock().tx.push_back(Frame{src:1, dst:2, bytes})   [socket layer]
net::poll()              -> for f in tx.drain(): if sockets[f.dst] exists -> its RX.push_back(f)
                                                  else dropped += 1               [loopback device]
sock2.recv()             -> NET.lock().sockets[2].pop_front()                     [socket layer]
```

The lock is taken and released within each call — never held across a `poll` and a
`recv`, so no re-entrancy or ordering hazard (single-core, non-IRQ).

## Edge cases

- **`recv` before `poll`** → `None` (device hasn't moved the frame — the observable
  proof the layers are separate).
- **Deliver to unbound `dst`** → dropped at `poll`, `dropped` counter++, no panic,
  other queues untouched.
- **Wrong-address socket** → its RX stays empty (delivery is keyed by `dst`).
- **`recv` on empty / drained / unbound** → `None`.
- **`bind` an already-bound addr** → `AddrInUse` (no silent queue clobber).
- **Payload `> MTU`** → `TooLong`, nothing queued.
- **Payload of length 0** → allowed (an empty frame is a valid frame); `recv`
  returns it, `payload.is_empty()`.
- **Self-test hygiene**: unbind every address it bound, so `NET` is clean for the
  shell command (which uses its own addresses and also cleans up).
- **Allocation**: `VecDeque`/`BTreeMap` growth allocates — fine in task context;
  no frame-allocator or IF-guarded-lock discipline needed (unlike `task::spawn`).

## Verification plan (proving, not observing)

- **Serial asserts** for every row of the matrix above. The delivered payload is
  asserted **byte-equal** to the sent bytes (Q4: real delivery, not a coincidental
  echo); `src`/`dst` on the received frame are asserted, so delivery is provably
  *by address*. The `recv`-before-`poll` `None` and the wrong-address `None` are the
  two guards that separate "modeled addressed delivery" from "echoed a buffer."
- **`M14: loopback online`** marker; bump `Makefile` `MILESTONE_MARKER`.
- **Live path**: the `net` shell built-in exercises the same `bind→send→poll→recv`
  interactively and prints the round-trip (drivable over `make console`).
- **Named risk (Q5)**: the only real risk is *scope gravity* (adding a header, a
  port, virtio). Mitigation: the deferral list is the firewall; any protocol byte is
  a new milestone. No kernel-mechanics stall risk (no hardware/asm/paging/IRQ).

## Not doing (and why it's fine)

No real NIC, so "hello over a socket" is a loopback round-trip, not an off-box
packet — which is exactly the stub PLAN §1 carves out of the networking non-goal.
The socket/interface/addressing abstraction is the whole learning payoff; a wire
would add protocol scope without adding to *that* lesson. virtqueues are worth a
future milestone on their own terms (a paravirtual device driver), framed as
device-driver learning, not networking.
