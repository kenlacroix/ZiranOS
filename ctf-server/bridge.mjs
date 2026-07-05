// Ziran OS — Tier-2 CTF bridge.
//
// One WebSocket connection => one fresh, ephemeral QEMU booting the kernel, with a
// UNIQUE flag planted only in that instance's RAM. The guest's serial console
// (COM1) is piped byte-for-byte to the socket, so the browser terminal drives the
// real kernel. Nothing about a session survives its disconnect.
//
// Security model (docs/planning/ctf-eng-plan.md §4): the player has full ring-0
// control of the guest — that's the challenge — so all isolation is OUTSIDE QEMU.
// This process never shells out with client bytes (argv only), spawns a hardened/
// sandboxed QEMU, and layers abuse controls (see CONFIG). The VM's firewall is the
// real blast-radius wall; these controls keep one host from being overwhelmed.
//
// The flag is a per-session secret passed via QEMU `-fw_cfg opt/flag` (never a shell
// line, never in the repo). TODO(kernel): read opt/flag at boot + the trusted-length
// OOB read; against today's kernel this boots and pipes serial, ignoring the flag.
//
//   node bridge.mjs        (config via env — see CONFIG)
// Requires `npm i` (ws) and qemu-system-x86_64 on PATH.

import { WebSocketServer } from 'ws';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';

const int = (v, d) => (v === undefined ? d : Number(v));
const CONFIG = {
  PORT:            int(process.env.PORT, 8080),
  HOST:           process.env.HOST ?? '127.0.0.1',       // bind localhost; cloudflared fronts it
  KERNEL:         process.env.KERNEL ?? '/opt/ziran/kernel-v86.bin',
  QEMU:           process.env.QEMU ?? 'qemu-system-x86_64',
  MEM_MB:         int(process.env.MEM_MB, 128),
  // --- rate / concurrency ---
  MAX_CONCURRENT: int(process.env.MAX_CONCURRENT, 12),   // total live QEMUs
  MAX_PER_IP:     int(process.env.MAX_PER_IP, 2),        // live QEMUs per client IP
  RATE_WINDOW_MS: int(process.env.RATE_WINDOW_MS, 60_000),
  RATE_MAX:       int(process.env.RATE_MAX, 10),         // new sessions / IP / window
  HOURLY_BUDGET:  int(process.env.HOURLY_BUDGET, 1000),  // global circuit breaker
  // --- per-session lifetime & I/O caps ---
  SESSION_MS:     int(process.env.SESSION_MS, 180_000),  // hard lifetime (3 min)
  IDLE_MS:        int(process.env.IDLE_MS, 90_000),      // kill after no client input
  MAX_MSG_BYTES:  int(process.env.MAX_MSG_BYTES, 262_144),      // reject huge single msgs
  MAX_IN_BYTES:   int(process.env.MAX_IN_BYTES, 4_194_304),     // total input per session
  MAX_WS_BACKLOG: int(process.env.MAX_WS_BACKLOG, 8_388_608),   // kill if client can't keep up
  // --- per-instance cgroup caps (via systemd-run when available) ---
  CPU_QUOTA:      process.env.CPU_QUOTA ?? '60%',
  MEM_MAX:        process.env.MEM_MAX ?? '256M',
  // --- gates ---
  PAUSE_FILE:     process.env.PAUSE_FILE ?? '/opt/ziran/PAUSED', // touch this to stop accepting
  TURNSTILE_SECRET: process.env.TURNSTILE_SECRET ?? '',   // set => require a Turnstile token
};

if (!existsSync(CONFIG.KERNEL)) {
  console.error(`[fatal] kernel not found: ${CONFIG.KERNEL} (set KERNEL=)`);
  process.exit(1);
}

// ---- shared counters / limiters (in-memory; single-process bridge) -------------
const metrics = { live: 0, spawned: 0, rejected: 0, startedAt: Date.now() };
const livePerIp = new Map();       // ip -> count
const rateHits = new Map();        // ip -> [timestamps]
const spawnTimes = [];             // global spawn timestamps (rolling hour)

const paused = () => process.env.PAUSE === '1' || existsSync(CONFIG.PAUSE_FILE);

function rateLimited(ip) {
  const now = Date.now();
  const hits = (rateHits.get(ip) ?? []).filter((t) => now - t < CONFIG.RATE_WINDOW_MS);
  hits.push(now); rateHits.set(ip, hits);
  return hits.length > CONFIG.RATE_MAX;
}
function overHourlyBudget() {
  const now = Date.now();
  while (spawnTimes.length && now - spawnTimes[0] > 3_600_000) spawnTimes.shift();
  return spawnTimes.length >= CONFIG.HOURLY_BUDGET;
}

function mintFlag() { return `FLAG{ziran-tier2-${randomBytes(8).toString('hex')}}`; }

// Optional Cloudflare Turnstile check — a human gate before we ever spawn a QEMU.
// No-op (allow) unless TURNSTILE_SECRET is set. Needs the client to pass ?t=<token>.
async function verifyTurnstile(token, ip) {
  if (!CONFIG.TURNSTILE_SECRET) return true;
  if (!token) return false;
  try {
    const r = await fetch('https://challenges.cloudflare.com/turnstile/v0/siteverify', {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ secret: CONFIG.TURNSTILE_SECRET, response: token, remoteip: ip }),
    });
    return (await r.json())?.success === true;
  } catch { return false; }
}

function buildQemuArgs(flag) {
  return [
    '-nodefaults', '-display', 'none',
    '-serial', 'stdio', '-monitor', 'none',
    '-m', String(CONFIG.MEM_MB),
    '-kernel', CONFIG.KERNEL,
    '-nic', 'none',
    '-accel', 'tcg,tb-size=256',
    '-no-reboot',
    '-sandbox', 'on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny',
    '-fw_cfg', `name=opt/flag,string=${flag}`,   // server-controlled; never client input
  ];
}
function spawnQemu(flag) {
  const qargs = buildQemuArgs(flag);
  // Per-instance cgroup caps via systemd-run when present; else a bare spawn.
  try {
    return spawn('systemd-run', [
      '--scope', '--quiet', '--collect',
      '-p', `MemoryMax=${CONFIG.MEM_MAX}`, '-p', `CPUQuota=${CONFIG.CPU_QUOTA}`, '-p', 'TasksMax=96',
      '--', CONFIG.QEMU, ...qargs,
    ], { stdio: ['pipe', 'pipe', 'pipe'] });
  } catch {
    return spawn(CONFIG.QEMU, qargs, { stdio: ['pipe', 'pipe', 'pipe'] });
  }
}

// ---- HTTP server: /health for monitoring; the WS upgrades on the same port ------
const server = createServer((req, res) => {
  if (req.url === '/health' || req.url === '/healthz') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      ok: !paused(), paused: paused(),
      live: metrics.live, max: CONFIG.MAX_CONCURRENT,
      spawned_total: metrics.spawned, rejected_total: metrics.rejected,
      hourly_spawns: spawnTimes.length, hourly_budget: CONFIG.HOURLY_BUDGET,
      uptime_s: Math.floor((Date.now() - metrics.startedAt) / 1000),
    }));
  } else { res.writeHead(404); res.end(); }
});
const wss = new WebSocketServer({ server, maxPayload: CONFIG.MAX_MSG_BYTES });
server.listen(CONFIG.PORT, CONFIG.HOST, () => {
  console.log(`[bridge] listening ${CONFIG.HOST}:${CONFIG.PORT}  kernel=${CONFIG.KERNEL}`);
  console.log(`[bridge] caps: total=${CONFIG.MAX_CONCURRENT} per-ip=${CONFIG.MAX_PER_IP} ` +
    `session=${CONFIG.SESSION_MS}ms idle=${CONFIG.IDLE_MS}ms rate=${CONFIG.RATE_MAX}/${CONFIG.RATE_WINDOW_MS}ms ` +
    `hourly=${CONFIG.HOURLY_BUDGET} turnstile=${CONFIG.TURNSTILE_SECRET ? 'on' : 'off'}`);
});

wss.on('connection', async (ws, req) => {
  const ip = (req.headers['cf-connecting-ip'] || req.socket.remoteAddress || '?').toString();
  const id = randomUUID().slice(0, 8);
  const log = (m) => console.log(`[${id}] ${ip} ${m}`);
  const reject = (reason) => { metrics.rejected++; log(`rejected: ${reason}`); try { ws.close(1013, reason); } catch {} };

  // ---- admission control (cheap checks first) --------------------------------
  if (paused())                                      return reject('paused for maintenance');
  if (overHourlyBudget())                            return reject('hourly capacity reached');
  if (rateLimited(ip))                               return reject('rate limited — slow down');
  if (metrics.live >= CONFIG.MAX_CONCURRENT)         return reject('server busy, try again');
  if ((livePerIp.get(ip) ?? 0) >= CONFIG.MAX_PER_IP) return reject('too many sessions from your IP');

  // ---- human gate (optional) --------------------------------------------------
  const token = new URL(req.url, 'http://x').searchParams.get('t');
  if (!(await verifyTurnstile(token, ip)))           return reject('human check failed');
  if (ws.readyState !== ws.OPEN)                     return; // client vanished during verify

  // ---- reserve a slot & spawn -------------------------------------------------
  metrics.live++; metrics.spawned++;
  livePerIp.set(ip, (livePerIp.get(ip) ?? 0) + 1);
  spawnTimes.push(Date.now());
  const flag = mintFlag();
  const qemu = spawnQemu(flag);
  log(`spawned (flag=${flag.slice(0, 18)}…), live=${metrics.live}`);

  let inBytes = 0;
  const kill = () => { try { qemu.kill('SIGKILL'); } catch {} };

  // hard lifetime + idle timers
  const hardTimer = setTimeout(() => {
    try { ws.send('\r\n[session ended — time limit reached]\r\n'); } catch {}
    log('killed: session timeout'); kill(); try { ws.close(1000, 'session timeout'); } catch {}
  }, CONFIG.SESSION_MS);
  let idleTimer;
  const bumpIdle = () => {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(() => {
      log('killed: idle'); kill(); try { ws.close(1000, 'idle timeout'); } catch {}
    }, CONFIG.IDLE_MS);
  };
  bumpIdle();

  // guest serial -> ws, with output backpressure (slow/stuck client => kill)
  qemu.stdout.on('data', (d) => {
    if (ws.readyState !== ws.OPEN) return;
    if (ws.bufferedAmount > CONFIG.MAX_WS_BACKLOG) {
      log('killed: ws backlog (slow client)'); kill(); try { ws.close(1009, 'output backlog'); } catch {}
      return;
    }
    ws.send(d);
  });
  qemu.stderr.on('data', (d) => log(`qemu stderr: ${d.toString().trim().slice(0, 200)}`));

  // ws -> guest stdin; bytes only, capped total, never a shell
  ws.on('message', (data, isBinary) => {
    bumpIdle();
    const buf = isBinary ? data : Buffer.from(data.toString());
    inBytes += buf.length;
    if (inBytes > CONFIG.MAX_IN_BYTES) {
      log('killed: input cap'); kill(); try { ws.close(1009, 'input limit'); } catch {}
      return;
    }
    if (qemu.stdin.writable) qemu.stdin.write(buf);
  });

  // ---- cleanup ---------------------------------------------------------------
  let cleaned = false;
  const cleanup = (why) => {
    if (cleaned) return; cleaned = true;
    clearTimeout(hardTimer); clearTimeout(idleTimer); kill();
    metrics.live = Math.max(0, metrics.live - 1);
    livePerIp.set(ip, Math.max(0, (livePerIp.get(ip) ?? 1) - 1));
    log(`cleaned (${why}), live=${metrics.live}`);
  };
  qemu.on('exit', (code) => { try { ws.close(1000, 'guest halted'); } catch {} cleanup(`qemu exit ${code}`); });
  ws.on('close', () => cleanup('ws close'));
  ws.on('error', () => cleanup('ws error'));
});

process.on('SIGTERM', () => { server.close(); process.exit(0); });
process.on('SIGINT', () => { server.close(); process.exit(0); });
