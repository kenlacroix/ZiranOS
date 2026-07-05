# Tier-2 CTF server (self-hosted, homelab)

The **remote** version of the filesystem CTF: a real Ziran kernel runs on a server,
a **unique flag lives only in that instance's RAM** (never in this repo, never in the
browser), and the only way to get it is to land the exploit over the wire. Develop
against the free in-browser [Tier 1](../web/ctf.html); capture here.

This directory is the **delivery infrastructure** — it is not kernel code and adds no
networking to the kernel (PLAN §8a). The kernel speaks only over its serial console,
exactly as under `make console`; the network lives entirely in this host-side bridge.

```
browser  ──wss──▶  Cloudflare Tunnel  ──▶  bridge.mjs  ──spawn──▶  QEMU ──▶ ziran kernel
 (xterm)          (no open ports,          (per-conn,             (serial only,
                   IP hidden)               limits, flag)          hardened, tcg)
```

## Status

Runnable skeleton. Transport + lifecycle + limits are complete and will boot today's
kernel and pipe its serial. **Two pieces are still to build** before it's a real
capture (both tracked in `docs/planning/ctf-eng-plan.md`):

1. **Kernel:** read the per-session flag from QEMU `fw_cfg` `opt/flag` at boot and
   plant it as the hidden secret; add the **trusted-length OOB read** vuln (Tier 1's
   in-image aliasing isn't strong enough — the secret must sit outside the player's
   bytes). See eng-plan §3.3 / §0.
2. **Client:** point the CTF page's WebSocket at `wss://<your-host>/` for a "remote"
   mode (the local Tier-1 sandbox stays as the practice target).

## What you need on Proxmox

- **A KVM VM, NOT a privileged LXC.** This is the whole security decision — you're
  inviting strangers to run ring-0 code, and a privileged LXC escape is host root.
  A VM gives you a second, different hypervisor boundary (guest escape → throwaway
  VM → still needs another escape to reach your host). Spec: **2 vCPU, 2–4 GB RAM,
  ~20 GB disk**, Debian 12 / Ubuntu 24.04 minimal.
- **No nested virtualization needed** — the inner QEMU uses `-accel tcg` (software
  emulation). Fast enough for a toy kernel and avoids exposing KVM as attack surface.

### Isolating the VM's network (the "own interface" question)

The VM needs **outbound internet** (so `cloudflared` can dial out) but must **not
reach your LAN**. So the control is a firewall rule, not just a separate NIC:

- **Recommended (no extra hardware):** give the NIC a **dedicated VLAN tag** on
  `vmbr0`, then in **Proxmox → VM → Firewall** add rules to DROP traffic to your LAN
  subnets and allow only DNS + outbound WAN. Enable the firewall at Datacenter, Node,
  and VM level.
- **Stronger (if you have a spare port):** a separate Linux bridge `vmbr1` on its own
  physical NIC into a different switch/VLAN — true L2 separation.
- Either way, `nftables.conf` here re-applies the **deny-LAN** policy *inside* the VM
  as defense in depth. Edit its `GATEWAY`/`LAN` vars to match your homelab.

A fully-isolated bridge with no uplink won't work on its own — the tunnel needs to
reach Cloudflare. The point is "WAN yes, LAN no," which the firewall does.

**Flat network (no VLANs)?** Then don't put the VM on `vmbr0` — that drops it onto
your LAN's L2 segment (adjacent to every host, one firewall typo from exposure).
Instead use **`proxmox-host-nat.conf`**: an internal-only bridge `vmbr1` (no uplink)
with the VM alone on a private subnet, and the **Proxmox host NATs it to the internet
while dropping all forwarding to LAN ranges**. That gives true L2 isolation *and*
outbound internet for the tunnel — a breakout lands on an island whose only exit is
WAN. Append that file's stanza to `/etc/network/interfaces` on the host, `ifreload
-a`, and verify from the VM that `curl https://cloudflare.com` works but a `ping` to
your NAS times out.

**Simpler alternative — VM on the LAN, egress blocked at the host.** If you'd rather
keep direct SSH (no jump host) and accept slightly less isolation, put the VM on
`vmbr0` and use **`proxmox-vm-firewall.fw`** (→ `/etc/pve/firewall/<vmid>.fw`). The
one rule that makes this safe: the egress block is enforced by the **Proxmox host
firewall, not inside the VM** — because a compromised (rooted) VM can flush its own
`nftables`, so the boundary must live outside it. Enable `ipfilter` (in that file) to
pin the VM to its MAC/IP against ARP/spoof tricks. This is weaker than the island
(the VM shares your L2 segment) but a sound, common choice at hobby scale — and it
makes admin/Claude inspection a plain `ssh ctf-admin@<vm-lan-ip>`.

## Install

```sh
# on the VM, as root:
scp -r ctf-server root@<vm>:/root/
ssh root@<vm> 'bash /root/ctf-server/setup-vm.sh'
# then the two steps it prints: copy the kernel, set up the cloudflared tunnel.
scp web/kernel-v86.bin root@<vm>:/opt/ziran/kernel-v86.bin
systemctl start ctf-bridge && journalctl -u ctf-bridge -f
```

`setup-vm.sh` installs QEMU + Node + cloudflared, creates an unprivileged `ctf`
user, loads the firewall, and installs the sandboxed `ctf-bridge` systemd service.

## Admin / Claude inspection access

You want to inspect the VM — but that's **inbound** (you → VM), a different direction
from the **outbound** VM → LAN pivot the island blocks. Allowing the first does *not*
reopen the second (stateful firewall: replies ride the established connection; the VM
still can't *initiate* to your LAN). So keep the island and add inbound admin only.

With the NAT'd design the VM has no LAN IP, so reach it through the **Proxmox host as
a jump host** — no port-forward, no new exposure:

```sh
ssh -J root@<proxmox-lan-ip> ctf-admin@10.66.66.2
```

`nftables.conf` already allows SSH only from the gateway `10.66.66.1` (where the jump
exits). Authorize your workstation's key on the VM's `ctf-admin` user (setup-vm.sh
prints the steps). **For Claude to inspect it non-interactively**, make every hop
key-based (your agent is forwarded by ProxyJump); then a session on your machine can
run e.g. `ssh -J root@<proxmox> ctf-admin@10.66.66.2 'journalctl -u ctf-bridge -n50'`.
Caveat: don't blindly trust a *compromised* VM's output in your shell — but the
isolation still blocks the real pivot, so inspection access doesn't reopen it.

## Defenses in place

| Layer | Control |
|---|---|
| Ingress | Cloudflare Tunnel — no open ports, home IP hidden, DDoS-fronted |
| Network island | VLAN + Proxmox firewall + in-VM `nftables` → **no LAN reachable** |
| QEMU | `-sandbox on` (seccomp), serial only, no NIC/USB/VGA, unprivileged user, tcg |
| Resources | per-instance cgroup caps (systemd-run), per-session timeout, concurrency + per-IP + rate caps |
| Bridge | never shells out with client bytes (argv only); flag via `fw_cfg`, not a shell line |
| Flag | unique per session, minted server-side, **never committed** (PLAN §3.7) |

## Abuse controls (rate limiting & friends)

Defense in depth, outermost first. The edge layer is the highest-leverage for a
public link — it stops floods *before* they cost you a QEMU spawn.

**Cloudflare edge** (in the dashboard; free tier — the tunnel already fronts it):
- **Turnstile** (recommended) — a human check before a session opens. Add the widget
  to the client, and set `TURNSTILE_SECRET` on the bridge; it verifies `?t=<token>`
  server-side and refuses to spawn without a valid one. Kills bot floods cold.
- **Rate-limiting rule** — cap requests per IP to the WS path at the edge.
- **Bot Fight Mode**, WAF managed rules, geo/ASN block, and **Under Attack Mode** as
  a break-glass.

**Bridge / per connection** (built into `bridge.mjs`, all env-tunable):
- concurrency cap (`MAX_CONCURRENT`), per-IP live cap (`MAX_PER_IP`), new-session
  rate limit (`RATE_MAX`/`RATE_WINDOW_MS`), and a global **hourly spawn budget**
  circuit breaker (`HOURLY_BUDGET`).
- hard session timeout (`SESSION_MS`) **and** idle timeout (`IDLE_MS`, reclaims
  abandoned tabs); max single-message size (`MAX_MSG_BYTES`); total input cap
  (`MAX_IN_BYTES`); **output backpressure** — a client too slow to drain output is
  killed (`MAX_WS_BACKLOG`) so it can't balloon memory.
- **kill switch:** `touch /opt/ziran/PAUSED` to stop accepting new sessions (no
  restart); **`GET /health`** returns live/total/rejected/paused for monitoring.

**Instance / host:** per-QEMU cgroup caps (CPU/mem/tasks via `systemd-run`), the
sandboxed service tree (`MemoryMax`/`CPUQuota`/`TasksMax` in the unit), and a
journald size cap so logs can't fill the disk. Consider `fail2ban` on the admin SSH.

Watch it: `curl -s localhost:8080/health | jq` on the VM, or via the jump host.

## Config

All via env / the systemd unit's `Environment=` lines (or a drop-in under
`/etc/systemd/system/ctf-bridge.service.d/`): `PORT`, `KERNEL`, `MAX_CONCURRENT`,
`MAX_PER_IP`, `SESSION_MS`, `IDLE_MS`, `RATE_MAX`, `HOURLY_BUDGET`, `MAX_MSG_BYTES`,
`MAX_IN_BYTES`, `MAX_WS_BACKLOG`, `MEM_MB`, `CPU_QUOTA`, `MEM_MAX`, `TURNSTILE_SECRET`,
`PAUSE_FILE`.

## Honest note

The eng-plan recommends a disposable **VPS** over the homelab precisely because a VPS
has *no LAN behind it* — but a hardened Proxmox VM with the network island above is a
sound, free choice while there's no traffic. If this ever gets popular, reconsider
moving it off your home network.
