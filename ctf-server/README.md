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

A fully-isolated bridge with no uplink won't work — the tunnel needs to reach
Cloudflare. The point is "WAN yes, LAN no," which the firewall does.

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

## Defenses in place

| Layer | Control |
|---|---|
| Ingress | Cloudflare Tunnel — no open ports, home IP hidden, DDoS-fronted |
| Network island | VLAN + Proxmox firewall + in-VM `nftables` → **no LAN reachable** |
| QEMU | `-sandbox on` (seccomp), serial only, no NIC/USB/VGA, unprivileged user, tcg |
| Resources | per-instance cgroup caps (systemd-run), per-session timeout, concurrency + per-IP + rate caps |
| Bridge | never shells out with client bytes (argv only); flag via `fw_cfg`, not a shell line |
| Flag | unique per session, minted server-side, **never committed** (PLAN §3.7) |

## Config

All via env / the systemd unit's `Environment=` lines (or a drop-in under
`/etc/systemd/system/ctf-bridge.service.d/`): `PORT`, `KERNEL`, `MAX_CONCURRENT`,
`MAX_PER_IP`, `SESSION_MS`, `RATE_MAX`, `MEM_MB`, `CPU_QUOTA`, `MEM_MAX`.

## Honest note

The eng-plan recommends a disposable **VPS** over the homelab precisely because a VPS
has *no LAN behind it* — but a hardened Proxmox VM with the network island above is a
sound, free choice while there's no traffic. If this ever gets popular, reconsider
moving it off your home network.
