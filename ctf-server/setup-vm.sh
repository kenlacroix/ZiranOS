#!/usr/bin/env bash
# Provision a Debian/Ubuntu VM to host the Ziran OS Tier-2 CTF bridge.
#
# Run as root INSIDE a fresh Proxmox KVM VM (NOT an LXC — see README.md). Idempotent.
# It installs QEMU + Node + cloudflared, creates an unprivileged `ctf` user, drops in
# the deny-LAN firewall, and installs the bridge as a hardened systemd service.
#
#   scp -r ctf-server root@vm:/root/ && ssh root@vm 'bash /root/ctf-server/setup-vm.sh'
#
# Then finish the two manual steps it prints at the end (kernel + tunnel).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CTF_USER=ctf
CTF_HOME=/opt/ziran

log() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }

[ "$(id -u)" -eq 0 ] || { echo "run as root"; exit 1; }

log "packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y --no-install-recommends \
  qemu-system-x86 nodejs npm nftables ca-certificates curl gnupg openssh-server

log "cloudflared (Cloudflare tunnel)"
if ! command -v cloudflared >/dev/null; then
  mkdir -p /usr/share/keyrings
  curl -fsSL https://pkg.cloudflare.com/cloudflare-main.gpg \
    | tee /usr/share/keyrings/cloudflare-main.gpg >/dev/null
  echo "deb [signed-by=/usr/share/keyrings/cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main" \
    > /etc/apt/sources.list.d/cloudflared.list
  apt-get update -y && apt-get install -y cloudflared
fi

log "unprivileged service user + app dir"
id -u "$CTF_USER" >/dev/null 2>&1 || useradd --system --home "$CTF_HOME" --shell /usr/sbin/nologin "$CTF_USER"
mkdir -p "$CTF_HOME"
cp "$HERE/bridge.mjs" "$HERE/package.json" "$CTF_HOME/"
( cd "$CTF_HOME" && npm install --omit=dev --no-audit --no-fund )
chown -R "$CTF_USER:$CTF_USER" "$CTF_HOME"

log "firewall: deny LAN, allow only DNS + outbound WAN (defense in depth)"
install -m 0644 "$HERE/nftables.conf" /etc/nftables.conf
systemctl enable --now nftables
nft -f /etc/nftables.conf
echo "  loaded /etc/nftables.conf — EDIT the LAN ranges in it to match your homelab."

log "host hygiene: cap journald so guest/bridge logs can't fill the disk"
mkdir -p /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/ctf.conf <<'EOF'
[Journal]
SystemMaxUse=500M
MaxRetentionSec=1week
EOF
systemctl restart systemd-journald || true

log "systemd service (sandboxed)"
install -m 0644 "$HERE/ctf-bridge.service" /etc/systemd/system/ctf-bridge.service
systemctl daemon-reload
systemctl enable ctf-bridge

cat <<EOF

\033[1;32mBase provisioning done.\033[0m Two manual steps remain:

1) Kernel binary — copy the (Tier-2) browser kernel to the VM:
     scp web/kernel-v86.bin root@<vm>:$CTF_HOME/kernel-v86.bin
   (Build it with 'make stage-web-kernel'. Until the Tier-2 kernel exists, the
    current kernel boots and pipes serial but ignores the injected flag.)

2) Cloudflare tunnel — expose ONLY the bridge, no open ports, IP hidden:
     cloudflared tunnel login
     cloudflared tunnel create ziran-ctf
     # route a hostname (e.g. ctf.yourdomain) to http://127.0.0.1:8080
     cloudflared tunnel route dns ziran-ctf ctf.yourdomain
     # run it as a service, ingress -> the bridge:
     cloudflared tunnel run --url http://127.0.0.1:8080 ziran-ctf   # or an install as a service

Then:  systemctl start ctf-bridge   &&   journalctl -u ctf-bridge -f
Point the browser client's WebSocket at wss://ctf.yourdomain/.

3) Admin / Claude inspection (INBOUND you->VM; the island is untouched):
     # on the VM, authorize your Mac's key for a non-root admin user:
     useradd -m -s /bin/bash ctf-admin && usermod -aG sudo ctf-admin
     install -d -m700 /home/ctf-admin/.ssh
     # paste your Mac's ~/.ssh/id_*.pub into:
     #   /home/ctf-admin/.ssh/authorized_keys   (chmod 600, chown ctf-admin)
   The in-VM firewall (nftables.conf) already allows SSH only from the gateway
   (10.66.66.1). Reach it from your Mac through the Proxmox host as a jump host:
     ssh -J root@<proxmox-lan-ip> ctf-admin@10.66.66.2
   Make every hop key-based so Claude can inspect non-interactively, e.g.:
     ssh -J root@<proxmox> ctf-admin@10.66.66.2 'systemctl status ctf-bridge; journalctl -u ctf-bridge -n50'
EOF
