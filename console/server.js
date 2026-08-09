#!/usr/bin/env node
// Ontolith console — zero-dependency management server.
// Serves the SPA from ./public and proxies whitelisted ontolith endpoints
// (/api/gw/* -> gateway, /api/mg/* -> management), injecting credentials
// from environment / .env. Listens on CONSOLE_BIND (default 127.0.0.1:8890).
import { createServer } from 'node:http';
import { readFileSync, existsSync, statSync } from 'node:fs';
import { join, normalize, extname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const PUBLIC = join(ROOT, 'public');

// ---- .env loader (no dependencies) ----
function loadEnv() {
  const envFile = join(ROOT, '.env');
  if (!existsSync(envFile)) return;
  for (const line of readFileSync(envFile, 'utf8').split('\n')) {
    const t = line.trim();
    if (!t || t.startsWith('#')) continue;
    const eq = t.indexOf('=');
    if (eq < 1) continue;
    const k = t.slice(0, eq).trim();
    let v = t.slice(eq + 1).trim();
    if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) {
      v = v.slice(1, -1);
    }
    if (process.env[k] === undefined) process.env[k] = v;
  }
}
loadEnv();

const cfg = {
  bind: process.env.CONSOLE_BIND || '127.0.0.1:8890',
  gateway: (process.env.ONTOLITH_GATEWAY_URL || 'http://127.0.0.1:8080').replace(/\/+$/, ''),
  management: (process.env.ONTOLITH_MANAGEMENT_URL || 'http://127.0.0.1:9091').replace(/\/+$/, ''),
  apiKey: process.env.ONTOLITH_API_KEY || '',
  tenant: process.env.ONTOLITH_TENANT || 'prod',
  user: process.env.ONTOLITH_USER || 'console',
  refreshMs: Number(process.env.CONSOLE_REFRESH_MS || 5000),
  bodyLimit: 2 * 1024 * 1024, // 2 MiB
};

if (!cfg.apiKey) {
  console.error('[ontolith-console] ONTOLITH_API_KEY is required (see .env.example)');
  process.exit(1);
}

// ---- whitelist: exact upstream path -> method(s) ----
const GW_ROUTES = new Map([
  ['health', ['GET']], ['ready', ['GET']], ['metrics', ['GET']], ['audit', ['GET']],
  ['sparql', ['GET', 'POST']], ['explain', ['GET', 'POST']],
  ['cluster', ['GET']], ['cluster/status', ['GET']], ['cluster/membership', ['GET']],
  ['cluster/shards', ['GET']], ['cluster/route', ['GET']], ['cluster/failover', ['GET']],
  ['semantic/search', ['GET']], ['semantic/index', ['POST']],
  ['data', ['POST']], ['data/nt', ['POST']], ['data/turtle', ['POST']],
  ['data/trig', ['POST']], ['data/nq', ['POST']],
]);
const MG_ROUTES = new Map([
  ['admin/health', ['GET']], ['admin/config', ['GET']], ['admin/layers', ['GET']],
  ['admin/monitoring', ['GET']], ['admin/traces', ['GET']], ['admin/data/stats', ['GET']],
  ['admin/data/audit', ['GET']],
  ['admin/data/replicate', ['POST']], ['admin/data/rebalance', ['POST']],
]);

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

function send(res, status, body, type = 'application/json; charset=utf-8') {
  res.writeHead(status, { 'Content-Type': type, 'Cache-Control': 'no-store' });
  res.end(body);
}

async function readBody(req) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > cfg.bodyLimit) {
      const err = new Error('body too large');
      err.status = 413;
      throw err;
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function proxy(req, res, upstream, path, method) {
  const url = `${upstream}/${path}${req.url.includes('?') ? '?' + req.url.split('?')[1] : ''}`;
  const headers = {
    'x-api-key': cfg.apiKey,
    'x-ontolith-tenant': cfg.tenant,
    'x-ontolith-user': cfg.user,
  };
  if (req.headers['content-type']) headers['content-type'] = req.headers['content-type'];
  let body;
  if (method === 'POST' || method === 'PUT' || method === 'PATCH') {
    body = await readBody(req);
    if (!headers['content-type']) headers['content-type'] = 'application/octet-stream';
  }
  let upstreamRes;
  try {
    upstreamRes = await fetch(url, {
      method,
      headers,
      body,
      redirect: 'manual',
      signal: AbortSignal.timeout(15000),
    });
  } catch (err) {
    return send(res, 502, JSON.stringify({ error: 'upstream unreachable', detail: String(err.message || err) }));
  }
  const text = await upstreamRes.text();
  const type = upstreamRes.headers.get('content-type') || 'application/json; charset=utf-8';
  send(res, upstreamRes.status, text, type);
}

function serveStatic(req, res, pathname) {
  const rel = pathname === '/' ? 'index.html' : pathname.replace(/^\/+/, '');
  const file = normalize(join(PUBLIC, rel));
  if (!file.startsWith(PUBLIC)) return send(res, 403, 'forbidden', 'text/plain; charset=utf-8');
  if (!existsSync(file) || !statSync(file).isFile()) return send(res, 404, 'not found', 'text/plain; charset=utf-8');
  send(res, 200, readFileSync(file), MIME[extname(file)] || 'application/octet-stream');
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://${cfg.bind}`);
  const method = req.method.toUpperCase();
  const pathname = decodeURIComponent(url.pathname);
  try {
    if (pathname.startsWith('/api/gw/')) {
      const path = pathname.slice('/api/gw/'.length);
      const allowed = GW_ROUTES.get(path);
      if (!allowed || !allowed.includes(method)) return send(res, 404, JSON.stringify({ error: 'no such gateway route' }));
      return await proxy(req, res, cfg.gateway, path, method);
    }
    if (pathname.startsWith('/api/mg/')) {
      const path = pathname.slice('/api/mg/'.length);
      const allowed = MG_ROUTES.get(path);
      if (!allowed || !allowed.includes(method)) return send(res, 404, JSON.stringify({ error: 'no such management route' }));
      return await proxy(req, res, cfg.management, path, method);
    }
    if (pathname === '/api/health') {
      const [gw, mg] = await Promise.all([
        fetch(`${cfg.gateway}/health`, { headers: gwHeaders(), signal: AbortSignal.timeout(4000) }).then(r => r.json()).catch(e => ({ status: 'unreachable', error: String(e.message || e) })),
        fetch(`${cfg.management}/admin/health`, { headers: gwHeaders(), signal: AbortSignal.timeout(4000) }).then(r => r.json()).catch(e => ({ status: 'unreachable', error: String(e.message || e) })),
      ]);
      return send(res, 200, JSON.stringify({ gateway: gw, management: mg, refresh_ms: cfg.refreshMs }));
    }
    if (pathname.startsWith('/api/')) return send(res, 404, JSON.stringify({ error: 'no such route' }));
    return serveStatic(req, res, pathname);
  } catch (err) {
    return send(res, err.status || 500, JSON.stringify({ error: String(err.message || err) }));
  }
});

function gwHeaders() {
  return {
    'x-api-key': cfg.apiKey,
    'x-ontolith-tenant': cfg.tenant,
    'x-ontolith-user': cfg.user,
  };
}

const [host, port] = cfg.bind.lastIndexOf(':') > -1 ? cfg.bind.split(':') : ['127.0.0.1', cfg.bind];
server.listen(Number(port), host, () => {
  console.log(`[ontolith-console] listening on http://${host}:${port}`);
  console.log(`[ontolith-console] gateway=${cfg.gateway} management=${cfg.management} tenant=${cfg.tenant} user=${cfg.user}`);
});
