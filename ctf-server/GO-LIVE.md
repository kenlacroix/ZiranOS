# Going public — Tier-2 CTF go-live runbook

Turning the (currently private, idle) VM into an internet-reachable capture. The
principle: **only the CTF bridge goes public; SSH/admin stays private, and the box
stays egress-locked.** A Cloudflare Tunnel dials *out*, so there is never an open
inbound port and your home IP is never exposed.

Do these in order — each gate protects the next.

## 0. Prereq — something real to expose

Don't publish a non-functional capture. Ship first:
- **Kernel:** read the per-session flag from QEMU `fw_cfg opt/flag` at boot + the
  **trusted-length OOB read** vuln (the secret must live outside the player's bytes —
  Tier 1's in-image aliasing is white-box). See `docs/planning/ctf-eng-plan.md` §3.3.
- **Client:** a "remote" WebSocket mode on the CTF page → `wss://<host>/`.
- Then `systemctl start ctf-bridge` and smoke-test locally over the bridge.

## 1. Harden the box for hostile traffic

- [ ] **Tighten egress.** Today outbound WAN is open; a breakout could use the box as
  an attack relay / exfil hop. Restrict the `output` chain in the VM's nftables to
  **Cloudflare (the tunnel) + cloud DNS + apt mirrors only**, dropping the rest. (Keep
  `established` accept so replies to your SSH still work.)
- [ ] **Proxmox host firewall (`.50`)** — now non-negotiable: a VM-root breakout can
  flush the in-VM firewall, so the host-enforced block is the real boundary. Apply
  `proxmox-vm-firewall.fw` (VM on LAN) or the NAT island. Needs SSH to `.50`.
- [x] **Automatic security updates** — `unattended-upgrades` (installed + enabled).
- [ ] **Monitoring** — watch the bridge `GET /health`; add an uptime check + alert.
      Keep the kill switch handy: `touch /opt/ziran/PAUSED` stops new sessions.

## 2. Abuse controls on

- [ ] **Turnstile** (the single best flood defense): add the widget to the client,
  set `TURNSTILE_SECRET` on the bridge (systemd `Environment=`). The bridge already
  verifies `?t=<token>` server-side and refuses to spawn without it.
- [ ] **Cloudflare rate-limiting rule** on the WS path + **Bot Fight Mode**.
- [ ] Tune the bridge caps for expected load: `MAX_CONCURRENT`, `MAX_PER_IP`,
  `RATE_MAX`, `SESSION_MS`, `IDLE_MS`, `HOURLY_BUDGET` (defaults are conservative).

## 3. The inbound path — Cloudflare Tunnel

- [ ] A hostname on your Cloudflare account (e.g. `ctf.kennethlacroix.me` if that zone
  is on CF — you already use CF for `ziranos.pages.dev`).
- [ ] On the VM (cloudflared is installed):
  ```sh
  cloudflared tunnel login                 # browser, your CF account
  cloudflared tunnel create ziran-ctf
  cloudflared tunnel route dns ziran-ctf ctf.yourdomain
  # ingress: ctf.yourdomain -> http://127.0.0.1:8080 (the bridge); run as a service
  sudo cloudflared service install <token>   # or a config.yml + systemd
  ```
- [ ] No firewall change needed for inbound — cloudflared dials out; the egress
  firewall already permits it. SSH stays LAN-only.
- [ ] Consider not exposing `/health` publicly (a Cloudflare path rule, or an Access
  policy) — it leaks live/spawn metrics.

## 4. Launch

- [ ] `systemctl start ctf-bridge`; confirm `journalctl -u ctf-bridge -f` is clean.
- [ ] End-to-end from a browser: connect over `wss://ctf.yourdomain/`, pass Turnstile,
  boot the kernel, land the exploit, capture a flag — and confirm the *same* image does
  **not** reveal that flag against the local Tier-1 sandbox (proves it's a real remote
  capture, not reading your own upload).
- [ ] Load-test: N concurrent sessions spawn + reap cleanly; a flood is rate-limited;
  a hung guest is killed on `SESSION_MS`.
- [ ] Then announce (lead with the live link). Keep `PAUSED` one `touch` away.

## Reality check

- Public surface = the bridge WS only. SSH/admin is not tunneled (LAN + key-only).
- Egress lock (tightened) means a breakout can't reach your LAN *or* relay out.
- Per-session unique flags + the honeytoken (§3.7) detect flag-sharing / shortcuts.
