# A remote pwn target

*Turning the toy kernel into a real remote CTF (Tier 2): a unique flag that lives
only in one server's RAM, and the far harder half of the project — building a
blast chamber where strangers can detonate exploits at me without taking the house
down with them.*

> New to this? The white-box version — [Tier 1](18-a-ctf-against-my-own-kernel.md) — runs
> the same kernel entirely in your browser, flag and all, as a sandbox to learn the
> bug in. Tier 2 is the graded exam: the flag is *not* in anything you're handed.
> Play it: [ziranos.pages.dev/ctf-remote](https://ziranos.pages.dev/ctf-remote).

Tier 1 was honest but soft. It ships you the whole thing — kernel, filesystem
image, the flag bytes sitting *right there* in the RAM disk if you know how to
craft the entry that reads them. That's a teaching sandbox: white-box on purpose,
because the point is to *understand* the length-confusion bug, not to prove you
pulled it off blind. It's a worksheet with the solutions printed upside-down at
the bottom.

Tier 2 is the version where I take the answer key away.

> **The lesson, up front: standing up a *safe* place for strangers to run exploits
> is far harder than the exploit. The bug is an afternoon. The isolation, the abuse
> controls, and an honest threat model are the month.**

## What makes it "real"

The whole difference is one property: **the flag is never in anything the player is
given.** Not in the page, not in the repo, not in the kernel image bytes, not in
the JavaScript. When you connect, the server mints a *unique* flag for your session
and injects it into that one QEMU instance's RAM at boot — through QEMU's `fw_cfg`
channel, the same firmware-config sidedoor real VMs use to pass in blobs. The
kernel reads it out of `fw_cfg` during early boot and stashes it in memory. Nowhere
else. There is exactly one copy, it exists only for the life of your connection,
and the only way to see it is to land the exploit *over the wire* and make the
guest print it down the serial line you're watching.

The vulnerability class is the same as Tier 1's — **length-confusion**: a bounds
check that trusts a length the attacker supplies, so "in bounds" is measured
against a number the attacker chose. Tier 1 lets you study exactly that flaw in the
open. Tier 2 makes you exploit it against a target whose flag you cannot read any
other way.

So I'll describe the *shape* and stop there: a length-confusion bug, a
per-session flag that lives only in the server's RAM. I'm deliberately **not**
publishing the crafted-image recipe, the offsets, or the precise reader flaw —
Tier 1 is the place to learn that, in white-box, where spoiling it is the point. If
I wrote the recipe here, the remote target would be a solved puzzle the day this
post shipped. The redaction is load-bearing, not coyness.

## One kernel, two personalities

The neat part on the kernel side is that it's the *same* binary in the browser and
on the server. A minimal `fw_cfg` reader runs at boot and asks the firmware whether
a per-session flag was handed in. In the browser there's no such channel, so the
answer is "no," and the kernel stays in its strict, Tier-1-safe configuration — the
deliberately-vulnerable mount path simply never engages. On the server, a flag *is*
present, and that presence is the switch that arms the vulnerable path. Same code;
the environment decides which personality boots.

`/kernel-review` earned its keep here and caught a hardening gap I'd have shipped.
The `fw_cfg` directory is read by first asking the device how many entries it has
and how big each is — and I trusted both numbers straight from the device, unbounded.
That's fine when the device is my launcher. It is *not* fine as a matter of house
style: a garbage or hostile `fw_cfg` device could hand back an enormous count and
turn early boot into a hang before the kernel ever reaches a console. The fix was
dull and correct — cap the entry count and the per-entry size — but the lesson is
the recurring one: **an early-boot reader trusting a length is the very bug the CTF
is about, one ring further down.** I nearly built the vulnerability into the thing
that reads the flag *about* the vulnerability.

That's the last I'll say about the kernel, because the kernel isn't the story. The
story is everything *around* it.

## The threat model that governs everything

Here is the fact that reorganizes the entire build: **the player has full ring-0
control of the guest. That's not a failure mode — that IS the challenge.** The
exploit's whole reward is code execution in the kernel. So I have to assume that
the moment someone succeeds, they own the guest completely: every in-guest check,
every "sandbox" flag inside the VM, every defensive line I could write in the
kernel is theirs to flip. In-guest defense is *meaningless* by construction.

Which means: **all isolation lives outside QEMU. The VM is the blast-radius wall,
and nothing inside it counts.**

This is the blast chamber. I'm inviting strangers to bring live charges and
detonate them at me. I cannot make the charge safe — making it detonate is the
game. All I can do is build a chamber whose walls hold, put exactly one thing worth
taking inside it, and make sure that when the charge goes off, the blast stops at
the wall. Every decision below is a wall of that chamber. None of them is in the
kernel.

## The bridge: one connection, one ephemeral VM

The front of the chamber is a small bridge service. One WebSocket connection spawns
one fresh, ephemeral, hardened QEMU, and pipes the guest's serial console back to
the browser byte-for-byte — you get a raw terminal onto a machine that was born
when you connected and dies when you leave. Each VM is:

- **Per-session**, with its own minted flag, so no two players share a target or a
  secret.
- **seccomp-sandboxed** — QEMU itself runs under a syscall filter, so even a QEMU
  bug has a short leash.
- **No NIC.** The guest has no network device at all. There is nothing to talk to.
- **Software emulation only** (TCG, no KVM), so the guest never touches
  hardware-virtualization surface on the host.

And because the real threat isn't a clever exploit but a hundred dumb ones at once,
the bridge is mostly *abuse controls*: per-IP caps, a concurrency ceiling, a
new-session rate limit, an hourly budget, idle timeouts *and* hard session
timeouts, output backpressure so a chatty guest can't drown the box, a kill switch,
and a `/health` endpoint so I can see it breathing. A public "run arbitrary code"
button is a DoS magnet; most of the code is there to make it a boring one.

## Homelab hardening: walls behind walls

The chamber sits in my homelab, and the hosting choices are all about what happens
*after* a breakout, not whether one occurs.

- **A real KVM VM, not a privileged LXC.** This one is non-negotiable and I want to
  be blunt about why: a privileged-LXC escape is *root on the host*. An LXC
  "container" shares the host kernel; a container escape and a VM escape are not the
  same weight class. The blast chamber has to be a genuine machine boundary, so it's
  a Proxmox KVM guest. The VM is allowed to be fully compromised. The host is not.
- **A Cloudflare Tunnel for ingress.** No open ports, no port-forward, my home IP
  never exposed. The box reaches *out* to Cloudflare; nothing reaches *in*. You
  can't attack a front door that isn't there.
- **Egress locked to almost nothing.** The box can reach *only* Cloudflare (the
  tunnel plus the CF-fronted apt mirrors) and DNS. This is the relay-proof wall:
  even a full breakout can't exfiltrate to an arbitrary host, can't phone home,
  can't pivot to the LAN, can't touch anything on my Proxmox network. Whatever a
  player becomes inside the chamber, it can talk to Cloudflare and nothing else.
- **A Proxmox *host* firewall as the durable boundary.** This is the subtle one. A
  VM-root attacker can flush the VM's *own* iptables — those rules live inside the
  blast radius. So the egress lock that actually matters is enforced at the
  *hypervisor*, outside the VM, where a compromised guest can't reach it. In-VM
  rules are convenience; the host firewall is the wall.
- **Turnstile as a human gate** in front of the whole thing, so a VM only spawns
  after Cloudflare's bot check passes — the abuse controls don't even get *asked*
  until something plausibly human is on the other end.
- **A static IP**, so all those pins — tunnel, firewall, allow-lists — don't drift
  out from under me on a DHCP lease renewal.

Walls behind walls. That's the whole design. Now the part where several of them
were wrong the first time.

## War stories

Because "it works now" is never the point. Here's what broke.

**1. The blank terminal.** The strongest failure and the one that best proves you
have to *drive the real thing*. The bridge runs as an unprivileged user, and I'd
wrapped each QEMU launch in `systemd-run --scope` for tidy per-session resource
control. Locally, as me, fine. Deployed, as the service user, every single QEMU
exited *before it started* — because `systemd-run --scope` needs polkit
authorization the unprivileged user didn't have. And the killer detail: my
try/catch only caught a *missing binary*. `systemd-run` was present and launched
cleanly; it just failed at *runtime*, a case my error handling sailed straight
past. So the bridge reported healthy, accepted the WebSocket, spawned "a VM," and
piped back… nothing. A blank terminal. **Time to find: too long, because I trusted
the health check.** I only caught it by driving the live tunnel end-to-end and
staring at an empty console that every automated signal swore was fine. The fix:
probe `systemd-run` *once* at startup, and if it can't scope, spawn QEMU directly
and skip the wrapper. The lesson is old and I re-learned it anyway — a green health
check is a claim, not a demo. Drive the real path.

**2. `apt` hung on the "Cloudflare-only" wall.** When I tightened egress to
Cloudflare ranges, package installs on the box started hanging. Obvious in
hindsight: `us.archive.ubuntu.com` resolves to *Canonical's own IP ranges*, not
Cloudflare's. My "only Cloudflare" allow-list was correct and had quietly firewalled
off the OS's own updates. Fix: allow Canonical's ranges too. The honest note is that
"lock egress to one provider" is never actually one provider — the CDN in front of
your ingress is not the CDN in front of your package mirror, and the wall you drew
on the whiteboard has a hole shaped like every dependency you forgot you had.

**3. Two conflicting COEP headers.** The main site sets a cross-origin-isolation
header site-wide (the browser emulator needs it). The remote CTF page had a
per-page exemption. The two together emitted *two* conflicting
`Cross-Origin-Embedder-Policy` headers on the remote page — and a browser resolving
that ambiguity the strict way could block the Turnstile iframe, which means the
human gate silently fails and no VM ever spawns. Fix: scope the isolation header to
*only* the emulator pages that need it, instead of setting it globally and carving
exceptions. Configuration-as-walls has the same failure mode as firewalls — a
broad rule plus a local exception is two rules that can contradict.

**4. The stale-kernel deploy trap. Again.** Same one that's bitten this project's
web deploys before: ship new content, forget the built kernel artifact is cached,
and serve a fresh page wrapped around a stale binary. I know about this trap. I have
*notes* on this trap. I hit it anyway, which is the most honest thing I can tell you
about deployment: the traps you've documented are not the traps you've stopped
falling into.

## What I deferred / didn't verify

In the spirit of not pretending the chamber is sealed:

- **I have not had a real adversary breach the guest and *test* the egress wall
  from inside.** I've reasoned it through and enforced it at the host, which is the
  right place — but "a compromised guest genuinely cannot exfiltrate" is an argument
  I've made, not an experiment I've run. The relay-proof claim is designed-for, not
  yet attacked.
- **The abuse controls are tuned by guesswork, not load.** The per-IP and hourly
  caps are first-guess numbers. I haven't run a real flood at it to find where the
  box actually tips over; the backpressure and timeouts are correct in shape,
  unproven in magnitude.
- **QEMU's own attack surface is a trust assumption.** seccomp plus no-NIC plus TCG
  is a serious leash, but a TCG or device-emulation bug in QEMU itself would be a
  wall I'm leaning on and haven't audited. The VM boundary is the strongest one I
  have; it is not one I built. And it's a single host — if the box falls over, the
  CTF is down. This is a hobby target, not a service; I'd rather name that than
  imply an SLA.

## Takeaway for the next person

If you're turning a toy into something strangers can attack over the internet:

- **Decide what's inside the blast radius before you write a line.** If the
  challenge *is* code execution in the guest, then nothing in the guest is a
  control. Draw the wall outside the thing you've conceded, and put your effort
  there. In-guest hardening on a target whose whole prize is ring-0 is theater.
- **VM, not privileged container.** A shared-kernel escape is host-root. If you're
  inviting exploitation, the boundary has to be a machine boundary.
- **Lock egress, and enforce it where the attacker can't reach.** In-guest firewall
  rules live inside the blast radius; a guest-root attacker flushes them. The rule
  that survives is the one on the hypervisor.
- **The secret must live only where the exploit can reach it.** A per-session flag
  injected into RAM and present in no artifact is the difference between a worksheet
  and an exam. If it's in the bytes you shipped, you shipped the answer.
- **Drive the live path.** A `/health` endpoint that returns 200 told me everything
  was fine while every VM died before starting. The blank terminal was the truth;
  the health check was a story.

The bug at the center of this is real and it's satisfying to land. But it took an
afternoon. The chamber around it — the VM boundary, the tunnel with no open ports,
the egress lock enforced at the host, the abuse controls, the human gate, and the
four things that broke before any of it held — took the rest of the time, and *that*
was the actual engineering. Making a thing exploitable is easy. Making a *safe place
to be exploited* is the work.

Come detonate something: [ziranos.pages.dev/ctf-remote](https://ziranos.pages.dev/ctf-remote).
The flag's in the RAM. It's yours if you can reach it.

---

*Built openly with Claude Code — the kernel, the bridge, the reviews that caught
the `fw_cfg` gap, and this post. The AI was in the loop the whole way; the story is
the engineering.*
