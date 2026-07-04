# A socket, from first principles

*Milestone 14 — what a socket actually is, demonstrated by the one interface where
"the network" is unambiguously just a buffer: loopback. No IP, no protocols, no
wire, no peer. The stretch-stub carve-out of the networking non-goal.*

Every other milestone in this project taught a mechanism the kernel genuinely
needs. This one is different: networking is an explicit **non-goal**, and the
roadmap carves out exactly one exception — a *stub* that shows what a socket is
underneath, and stops there. So the honest framing up front is not "the kernel can
network now" (it can't, and won't). It's: **here is the smallest thing that teaches
the socket abstraction, and here is the firewall that keeps it from growing into a
networking stack.**

## The problem: a function call is the wrong shape

When one part of a program wants to hand data to another, it calls a function. The
caller knows the callee, holds a reference to it, and control returns when the
callee is done. That shape is so natural it's invisible — and it is exactly what a
network is *not*.

A network endpoint has three properties a function call collapses:

- **It's named, not referenced.** You send to an *address*, not to an object you
  hold. The sender need not know who — or whether anyone — is bound to it.
- **It's decoupled from the peer.** Sending doesn't run the receiver. The bytes sit
  in a queue until the other side chooses to read them (or never does).
- **A device sits in the middle.** The sender hands bytes to an *interface*; the
  interface delivers them. Those are two separate steps, not one call.

A **socket** is the handle that gives you those three properties: a named endpoint
with a receive queue, reached by address, decoupled from whoever is on the other
end. Milestone 14 builds exactly that and nothing else.

## The smallest interface that shows it: loopback

You don't need a wire, a protocol, or a second machine to demonstrate a socket.
You need one **interface** whose transmit path *is* its receive path — loopback.
What you send comes back in on the same interface, routed to whichever local socket
owns the destination address. It's the one device where "the network" is provably
just a buffer, so the abstraction stands on its own with no peer.

Ziran's loopback (`src/net.rs`) has three moving parts behind one lock:

```text
  Frame { src: u16, dst: u16, payload }   -- who from, who to, and the bytes
  tx:      a transmit queue                -- frames sent but not yet delivered
  sockets: address -> a receive queue      -- one per bound socket
```

Addresses are bare `u16`s. That is the whole "addressing scheme" — enough to teach
"delivery is by address," with no IP, no ports, no structure to parse.

## The three layers, kept separate on purpose

The lesson lives in the fact that a send does **not** touch the receiver. Three
calls, three layers:

```text
  socket layer    sock.send(to, bytes)   -> just appends a Frame to the device's tx queue
  device layer    net::poll()            -> drains tx, routes each Frame to sockets[dst]'s rx
  socket layer    sock.recv()            -> pops a Frame off *this* socket's rx queue
```

`send` returns immediately, having only queued the frame on the device. Nothing is
delivered until `poll()` — the loopback device step — runs and moves each frame
from the transmit queue to the receive queue of the socket bound to its
destination. That decoupling is exactly a real NIC's TX/RX split, and it's what
makes the abstraction observable: **`recv()` returns `None` until `poll()` runs,**
because the frame is genuinely in flight between the two layers.

"Loopback" is the routing rule: transmit becomes receive, delivered to the local
socket whose address matches `dst`. No frame leaves the machine; none needs to, to
show what a socket is.

## Verification honesty: addressed delivery vs. an echo

The trap here is a demo that *looks* like networking but is really just "I put
bytes in a buffer and got them back" — an echo, which teaches nothing about
addressing. Four assertions in `net::self_test()` separate the two:

- **In flight until polled.** `recv()` is `None` after `send` and before `poll` —
  proof the socket and device layers are distinct, not one call.
- **Byte-exact, addressed.** After `poll`, the receiver gets the exact payload with
  `src`/`dst` preserved — delivery carries addressing, it doesn't just move bytes.
- **By address, not broadcast.** A socket bound to a *different* address receives
  nothing. If it did, "delivery" would be "shout to everyone," not addressing.
- **Unbound destinations drop cleanly.** A frame for an address no socket holds is
  dropped and counted (no panic, no error to the sender) — a real link doesn't know
  the peer is gone.

Plus the small totality checks: binding a live address twice is `AddrInUse` (no
silent clobber), `recv` on a drained queue is `None` (a real would-block, not a
latched value), and a payload past `MTU` is `TooLong`. The `net` shell command runs
the same round-trip live.

## The honest limitations (this is a stub, and stays one)

Everything a "real" network has is deliberately absent, and each absence is a
firewall against the project's single biggest risk — scope creep toward a
networking stack:

- **No protocols.** No IP, TCP, UDP, ports, checksums, or ARP. Addresses are bare
  integers; a payload is opaque bytes. The moment a protocol header appears, that's
  a new milestone, not "while I'm here."
- **No real device.** No virtio-net, no PCI, no DMA, no QEMU `-netdev`. The kernel
  boots with no network at all.
- **No peer.** "Hello over a socket" is a loopback round-trip, not an off-box
  packet — which is exactly the stub the non-goal carves out.
- **No backpressure.** `MTU` bounds a single frame, but queue depth is unbounded; a
  disciplined caller drains. A ring-buffer cap would be the first step toward a real
  driver.

## Where this goes next

Two clean future exercises, each on its own terms, neither a gap in the roadmap:

- **A paravirtual device driver (virtio-net).** PCI enumeration, virtqueues, DMA
  rings — genuinely instructive, but as *device-driver* learning, framed that way,
  not smuggled in under "networking." Virtqueues are how every virtio device works
  (disk, net, console), so it's a broadly useful milestone whenever it's taken up.
- **A protocol, if ever.** Even a toy IP/UDP layer on top of loopback would teach
  headers and demultiplexing — but it's squarely the "networking stack" non-goal,
  so it stays a *deliberately declined* direction, recorded here so a future self
  doesn't re-litigate it.

The socket/interface/addressing abstraction was the whole learning payoff, and a
wire would add protocol scope without adding to *that* lesson. That restraint —
building the stub and refusing the stack — is as much the milestone as the code.
