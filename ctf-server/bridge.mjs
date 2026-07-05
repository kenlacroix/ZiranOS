// Ziran OS — Tier-2 CTF bridge.
//
// One WebSocket connection => one fresh, ephemeral QEMU booting the kernel, with a
// UNIQUE flag planted only in that instance's RAM. The guest's serial console
// (COM1) is piped byte-for-byte to the socket, so the browser terminal drives the
// real kernel. Nothing about a session survives its disconnect.
//
// Security model (see docs/planning/ctf-eng-plan.md §4): the player has full ring-0
// control of the guest — that's the challenge — so all isolation is OUTSIDE QEMU.
// This process therefore: never shells out with client bytes (argv arrays only),
// spawns a hardened/sandboxed QEMU, caps concurrency + lifetime + rate, and relies
// on the VM's VLAN firewall (nftables.conf) as the real blast-radius wall.
//
// The flag is a per-session secret passed to the guest via QEMU `-fw_cfg` (never on
// a shell line, never in the repo). The Tier-2 kernel must read `opt/flag` at boot
// and plant it as its hidden secret. TODO(kernel): that read + the trusted-length
// OOB read are the kernel-side work this bridge is waiting on; against today's
// kernel this still boots and pipes serial, it just ignores the flag.
//
//   node bridge.mjs      (config via env — see CONFIG below)
//
// Requires: `npm i` (the `ws` package) and `qemu-system-x86_64` on PATH.

import { WebSocketServer } from 'ws';
import { spawn } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';

const CONFIG = {
  PORT:            Number(process.env.PORT            ?? 8080),
  HOST:           process.env.HOST                   ?? '127.0.0.1', // bind localhost; cloudflared fronts it
  KERNEL:         process.env.KERNEL                 ?? '/opt/ziran/kernel-v86.bin',
  QEMU:           process.env.QEMU                   ?? 'qemu-system-x86_64',
  MEM_MB:         Number(process.env.MEM_MB          ?? 128),
  MAX_CONCURRENT: Number(process.env.MAX_CONCURRENT  ?? 12),   // total live QEMUs
  MAX_PER_IP:     Number(process.env.MAX_PER_IP      ?? 2),    // live QEMUs per client IP
  SESSION_MS:     Number(process.env.SESSION_MS      ?? 180_000), // hard lifetime (3 min)
  RATE_WINDOW_MS: Number(process.env.RATE_WINDOW_MS  ?? 60_000),
  RATE_MAX:       Number(process.env.RATE_MAX        ?? 10),   // new sessions / IP / window
  // Per-instance cgroup caps via systemd-run, when available (see resourceWrap()).
  CPU_QUOTA:      process.env.CPU_QUOTA              ?? '60%',
  MEM_MAX:        process.env.MEM_MAX                ?? '256M',
};

if (!existsSync(CONFIG.KERNEL)) {
  console.error(`[fatal] kernel not found: ${CONFIG.KERNEL} (set KERNEL=)`);
  process.exit(1);
}

// ---- lightweight limiters (in-memory; fine for a single-process bridge) --------
let liveTotal = 0;
const livePerIp = new Map();       // ip -> count
const rateHits = new Map();        // ip -> [timestamps]

function rateLimited(ip) {
  const now = Date.now();
  const hits = (rateHits.get(ip) ?? []).filter((t) => now - t < CONFIG.RATE_WINDOW_MS);
  hits.push(now);
  rateHits.set(ip, hits);
  return hits.length > CONFIG.RATE_MAX;
}

// A fresh, unique flag per session — server-side only, never committed.
function mintFlag() {
  return `FLAG{ziran-tier2-${randomBytes(8).toString('hex')}}`;
}

// Wrap the QEMU argv in systemd-run for per-instance cgroup caps if available;
// otherwise run it directly (the VM-level limits still apply). Never includes any
// client-controlled string.
function buildCommand(flag) {
  const qemuArgs = [
    '-nodefaults',
    '-display', 'none',
    '-serial', 'stdio',           // COM1 -> our stdio pipe
    '-monitor', 'none',           // no QEMU monitor reachable from the guest/serial
    '-m', String(CONFIG.MEM_MB),
    '-kernel', CONFIG.KERNEL,
    '-nic', 'none',               // no network device in the guest at all
    '-accel', 'tcg,tb-size=256',  // software emulation — no KVM surface
    '-no-reboot',
    // Seccomp-sandbox the QEMU process itself (defense in depth).
    '-sandbox', 'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny',
    // Per-session secret, delivered to the guest out-of-band (kernel reads opt/flag).
    '-fw_cfg', `name=opt/flag,string=${flag}`,
  ];
  return { cmd: CONFIG.QEMU, args: qemuArgs };
}

function resourceWrap(cmd, args) {
  // systemd-run --scope applies cgroup limits to the child. If it's missing, fall
  // back to a bare spawn (VM-level caps + our timeout still bound it).
  return {
    cmd: 'systemd-run',
    args: [
      '--scope', '--quiet', '--collect',
      '-p', `MemoryMax=${CONFIG.MEM_MAX}`,
      '-p', `CPUQuota=${CONFIG.CPU_QUOTA}`,
      '-p', 'TasksMax=96',
      '--', cmd, ...args,
    ],
    fallback: { cmd, args },
  };
}

const wss = new WebSocketServer({ host: CONFIG.HOST, port: CONFIG.PORT });
console.log(`[bridge] listening ws://${CONFIG.HOST}:${CONFIG.PORT}  kernel=${CONFIG.KERNEL}`);
console.log(`[bridge] caps: total=${CONFIG.MAX_CONCURRENT} per-ip=${CONFIG.MAX_PER_IP} ` +
            `session=${CONFIG.SESSION_MS}ms rate=${CONFIG.RATE_MAX}/${CONFIG.RATE_WINDOW_MS}ms`);

wss.on('connection', (ws, req) => {
  const ip = (req.headers['cf-connecting-ip'] || req.socket.remoteAddress || '?').toString();
  const id = randomUUID().slice(0, 8);
  const log = (m) => console.log(`[${id}] ${ip} ${m}`);

  // ---- admission control ------------------------------------------------------
  if (rateLimited(ip)) { ws.close(1013, 'rate limited — slow down'); log('rejected: rate'); return; }
  if (liveTotal >= CONFIG.MAX_CONCURRENT) { ws.close(1013, 'server busy, try again'); log('rejected: full'); return; }
  if ((livePerIp.get(ip) ?? 0) >= CONFIG.MAX_PER_IP) { ws.close(1013, 'too many sessions from your IP'); log('rejected: per-ip'); return; }

  liveTotal++;
  livePerIp.set(ip, (livePerIp.get(ip) ?? 0) + 1);

  const flag = mintFlag();
  const base = buildCommand(flag);
  let plan = resourceWrap(base.cmd, base.args);
  let qemu;
  try {
    qemu = spawn(plan.cmd, plan.args, { stdio: ['pipe', 'pipe', 'pipe'] });
  } catch {
    // systemd-run unavailable — run QEMU directly.
    plan = { fallback: plan.fallback };
    qemu = spawn(plan.fallback.cmd, plan.fallback.args, { stdio: ['pipe', 'pipe', 'pipe'] });
  }
  log(`spawned qemu (flag=${flag.slice(0, 18)}…), live=${liveTotal}`);

  // ---- wiring: guest serial <-> websocket ------------------------------------
  qemu.stdout.on('data', (d) => { if (ws.readyState === ws.OPEN) ws.send(d); });
  qemu.stderr.on('data', (d) => log(`qemu stderr: ${d.toString().trim().slice(0, 200)}`));
  ws.on('message', (data, isBinary) => {
    // Client keystrokes -> guest stdin. Bytes only; never parsed, never a shell.
    if (qemu.stdin.writable) qemu.stdin.write(isBinary ? data : Buffer.from(data.toString()));
  });

  // ---- lifecycle: timeout + cleanup ------------------------------------------
  const kill = () => { try { qemu.kill('SIGKILL'); } catch {} };
  const deadline = setTimeout(() => {
    try { ws.send('\r\n[session ended — 3 min limit reached]\r\n'); } catch {}
    log('killed: session timeout'); kill(); ws.close(1000, 'session timeout');
  }, CONFIG.SESSION_MS);

  let cleaned = false;
  const cleanup = (why) => {
    if (cleaned) return; cleaned = true;
    clearTimeout(deadline); kill();
    liveTotal = Math.max(0, liveTotal - 1);
    livePerIp.set(ip, Math.max(0, (livePerIp.get(ip) ?? 1) - 1));
    log(`cleaned (${why}), live=${liveTotal}`);
  };
  qemu.on('exit', (code) => { try { ws.close(1000, 'guest halted'); } catch {} cleanup(`qemu exit ${code}`); });
  ws.on('close', () => cleanup('ws close'));
  ws.on('error', () => cleanup('ws error'));
});

process.on('SIGTERM', () => { wss.close(); process.exit(0); });
process.on('SIGINT', () => { wss.close(); process.exit(0); });
