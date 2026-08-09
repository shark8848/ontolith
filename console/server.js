#!/usr/bin/env node
// Ontolith console — zero-dependency management server.
// Serves the Vite-built SPA from ./dist (falling back to ./src in dev) and
// proxies whitelisted ontolith endpoints, injecting per-cluster credentials.
// Features: multi-cluster, history sampler, zero-dep gRPC channel, optional
// TLS + access token.
import { createServer as createHttpServer } from 'node:http';
import { createServer as createHttpsServer } from 'node:https';
import { readFileSync, existsSync, statSync } from 'node:fs';
import { join, normalize, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { callGrpc, PATHS, encode, decode } from './grpc.js';

const ROOT = fileURLToPath(new URL('.', import.meta.url));
const PUBLIC = process.env.CONSOLE_STATIC_DIR
  ? join(ROOT, process.env.CONSOLE_STATIC_DIR)
  : join(ROOT, existsSync(join(ROOT, 'dist')) ? 'dist' : 'src');

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
    if ((v.startsWith('"') && v.endsWith('"')) || (v.startsWith("'") && v.endsWith("'"))) v = v.slice(1, -1);
    if (process.env[k] === undefined) process.env[k] = v;
  }
}
loadEnv();

const cfg = {
  bind: process.env.CONSOLE_BIND || '127.0.0.1:8890',
  refreshMs: Math.max(1000, Number(process.env.CONSOLE_REFRESH_MS || 5000)),
  historyPoints: Math.max(10, Number(process.env.CONSOLE_HISTORY_POINTS || 120)),
  bodyLimit: 2 * 1024 * 1024,
  proxyTimeoutMs: Math.max(5000, Number(process.env.CONSOLE_PROXY_TIMEOUT_MS || 60000)),
  tlsCert: process.env.CONSOLE_TLS_CERT || '',
  tlsKey: process.env.CONSOLE_TLS_KEY || '',
  authToken: process.env.CONSOLE_AUTH_TOKEN || '',
};

// ---- clusters ----
function loadClusters() {
  const file = join(ROOT, 'clusters.json');
  if (existsSync(file)) {
    const list = JSON.parse(readFileSync(file, 'utf8'));
    if (!Array.isArray(list) || list.length === 0) throw new Error('clusters.json must be a non-empty array');
    for (const c of list) {
      if (!c.id || !c.gateway || !c.apiKey) throw new Error(`cluster ${c.id || '?'} missing id/gateway/apiKey`);
      c.management = c.management || '';
      c.grpc = c.grpc || '';
    }
    return list;
  }
  const apiKey = process.env.ONTOLITH_API_KEY || '';
  if (!apiKey) {
    console.error('[ontolith-console] ONTOLITH_API_KEY (or clusters.json) is required — see .env.example / clusters.example.json');
    process.exit(1);
  }
  return [{
    id: 'default',
    name: process.env.ONTOLITH_CLUSTER_NAME || 'Production',
    gateway: (process.env.ONTOLITH_GATEWAY_URL || 'http://127.0.0.1:8080').replace(/\/+$/, ''),
    management: (process.env.ONTOLITH_MANAGEMENT_URL || 'http://127.0.0.1:9091').replace(/\/+$/, ''),
    grpc: (process.env.ONTOLITH_GRPC_URL || 'http://127.0.0.1:50051').replace(/\/+$/, ''),
    apiKey,
    tenant: process.env.ONTOLITH_TENANT || 'prod',
    user: process.env.ONTOLITH_USER || 'console',
  }];
}
const clusters = loadClusters();
const byId = new Map(clusters.map(c => [c.id, c]));
const defaultCluster = clusters[0];

// ---- whitelist: upstream path -> method(s) ----
const GW_ROUTES = new Map([
  ['health', ['GET']], ['ready', ['GET']], ['metrics', ['GET']], ['audit', ['GET']],
  ['sparql', ['GET', 'POST']], ['explain', ['GET', 'POST']],
  ['cluster', ['GET']], ['cluster/status', ['GET']], ['cluster/membership', ['GET']],
  ['cluster/shards', ['GET']], ['cluster/route', ['GET']], ['cluster/failover', ['GET']],
  ['semantic/search', ['GET']], ['semantic/index', ['POST']],
  ['inference', ['GET']], ['validate/shacl', ['POST']], ['materialize', ['POST']],
  ['data', ['POST']], ['data/nt', ['POST']], ['data/turtle', ['POST']],
  ['data/trig', ['POST']], ['data/nq', ['POST']],
]);
const MG_ROUTES = new Map([
  ['admin/health', ['GET']], ['admin/config', ['GET']], ['admin/layers', ['GET']],
  ['admin/monitoring', ['GET']], ['admin/traces', ['GET']], ['admin/data/stats', ['GET']],
  ['admin/plugins', ['GET']],
  ['admin/data/audit', ['GET']],
  ['admin/data/replicate', ['POST']], ['admin/data/rebalance', ['POST']],
]);

const MIME = {
  '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8', '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon',
};

function send(res, status, body, type = 'application/json; charset=utf-8') {
  res.writeHead(status, { 'Content-Type': type, 'Cache-Control': 'no-store' });
  res.end(body);
}
const sendJson = (res, status, obj) => send(res, status, JSON.stringify(obj));
function gwHeaders(c) {
  return { 'x-api-key': c.apiKey, 'x-ontolith-tenant': c.tenant, 'x-ontolith-user': c.user };
}
async function readBody(req) {
  const chunks = []; let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > cfg.bodyLimit) { const e = new Error('body too large'); e.status = 413; throw e; }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

async function proxy(req, res, cluster, path, method) {
  const upstream = path.startsWith('admin/') ? cluster.management : cluster.gateway;
  if (!upstream) return sendJson(res, 400, { error: 'cluster has no ' + (path.startsWith('admin/') ? 'management' : 'gateway') + ' url' });
  const url = `${upstream}/${path}${req.url.includes('?') ? '?' + req.url.split('?')[1] : ''}`;
  const headers = gwHeaders(cluster);
  if (req.headers['content-type']) headers['content-type'] = req.headers['content-type'];
  let body;
  if (method === 'POST' || method === 'PUT' || method === 'PATCH') {
    body = await readBody(req);
    if (!headers['content-type']) headers['content-type'] = 'application/octet-stream';
  }
  let up;
  try {
    up = await fetch(url, { method, headers, body, redirect: 'manual', signal: AbortSignal.timeout(cfg.proxyTimeoutMs) });
  } catch (err) {
    return sendJson(res, 502, { error: 'upstream unreachable', detail: String(err.message || err) });
  }
  const text = await up.text();
  send(res, up.status, text, up.headers.get('content-type') || 'application/json; charset=utf-8');
}

async function grpcCall(cluster, method, payload) {
  if (!cluster.grpc) throw Object.assign(new Error('cluster has no grpc url'), { status: 400 });
  const meta = gwHeaders(cluster);
  if (method === 'health') {
    const buf = await callGrpc({ grpcUrl: cluster.grpc, path: PATHS.health, message: encode.health(), metadata: meta });
    return decode.health(buf);
  }
  const q = payload.query || '';
  if (!q.trim()) throw Object.assign(new Error('query is required'), { status: 400 });
  const msg = encode.query({
    query: q,
    format: 'json',
    explain: !!payload.explain,
    timeoutMs: payload.timeoutMs || 10000,
    consistency: payload.consistency || 'strong',
  });
  const buf = await callGrpc({ grpcUrl: cluster.grpc, path: PATHS.query, message: msg, metadata: meta });
  const r = decode.query(buf);
  if (!r.ok || r.http_status >= 400) {
    throw Object.assign(new Error(r.error || `gRPC HTTP ${r.http_status}`), { status: r.http_status || 502 });
  }
  try { return JSON.parse(r.body); } catch { return { raw: r.body }; }
}

// ---- history sampler ----
const history = new Map(clusters.map(c => [c.id, []]));
async function sample(cluster) {
  const rec = { ts: Date.now(), cluster: cluster.id };
  const [h, m] = await Promise.all([
    fetch(`${cluster.gateway}/health`, { headers: gwHeaders(cluster), signal: AbortSignal.timeout(4000) }).then(r => r.json()).catch(() => null),
    cluster.management ? fetch(`${cluster.management}/admin/monitoring`, { headers: gwHeaders(cluster), signal: AbortSignal.timeout(4000) }).then(r => r.json()).catch(() => null) : Promise.resolve(null),
  ]);
  if (h) { rec.triples = h.triples ?? 0; rec.quads = h.quads ?? 0; rec.status = h.status || 'unknown'; }
  if (m) {
    rec.requests_total = m.requests_total ?? 0; rec.sparql_total = m.sparql_total ?? 0;
    rec.sparql_errors = m.sparql_errors ?? 0; rec.ingest_total = m.ingest_total ?? 0;
    rec.latency_avg_ms = m.latency_avg_ms ?? 0;
    if (m.cluster) { rec.commit_index = m.cluster.commit_index ?? 0; rec.healthy = m.cluster.healthy ?? 0; rec.nodes = m.cluster.nodes ?? 0; }
    rec.audit_events = m.audit_events;
  }
  const arr = history.get(cluster.id);
  arr.push(rec);
  if (arr.length > cfg.historyPoints) arr.shift();
}
for (const c of clusters) { sample(c); }
setInterval(() => { for (const c of clusters) sample(c).catch(() => {}); }, cfg.refreshMs);

// ---- auth guard ----
function authorized(req) {
  if (!cfg.authToken) return true;
  const h = req.headers['authorization'] || '';
  return h === 'Bearer ' + cfg.authToken;
}

// ---- static ----
function serveStatic(req, res, pathname) {
  const rel = pathname === '/' ? 'index.html' : pathname.replace(/^\/+/, '');
  const file = normalize(join(PUBLIC, rel));
  if (!file.startsWith(PUBLIC)) return send(res, 403, 'forbidden', 'text/plain; charset=utf-8');
  if (!existsSync(file) || !statSync(file).isFile()) return send(res, 404, 'not found', 'text/plain; charset=utf-8');
  send(res, 200, readFileSync(file), MIME[extname(file)] || 'application/octet-stream');
}

// ---- router ----
async function route(req, res) {
  const url = new URL(req.url, `http://${cfg.bind}`);
  const method = req.method.toUpperCase();
  const pathname = decodeURIComponent(url.pathname);
  // Static assets stay public so the SPA can render its login overlay; the
  // access token guards the API surface only.
  if (pathname.startsWith('/api/') && !authorized(req)) {
    return sendJson(res, 401, { error: 'unauthorized: missing/invalid console token' });
  }

  if (pathname === '/api/clusters') {
    return sendJson(res, 200, clusters.map(({ id, name, gateway, management, grpc, tenant, user }) => ({ id, name, gateway, management, grpc, tenant, user })));
  }
  if (pathname === '/api/health') {
    const only = url.searchParams.get('cluster');
    const list = only && only !== 'all' ? [byId.get(only)].filter(Boolean) : clusters;
    const results = await Promise.all(list.map(async c => {
      const [h, m] = await Promise.all([
        fetch(`${c.gateway}/health`, { headers: gwHeaders(c), signal: AbortSignal.timeout(4000) }).then(r => r.json()).catch(e => ({ status: 'unreachable', error: String(e.message || e) })),
        c.management ? fetch(`${c.management}/admin/health`, { headers: gwHeaders(c), signal: AbortSignal.timeout(4000) }).then(r => r.json()).catch(e => ({ status: 'unreachable', error: String(e.message || e) })) : Promise.resolve(null),
      ]);
      return { cluster: c.id, name: c.name, gateway: h, management: m };
    }));
    return sendJson(res, 200, only ? results[0] || { error: 'no such cluster' } : results);
  }
  if (pathname === '/api/history') {
    const id = url.searchParams.get('cluster') || defaultCluster.id;
    return sendJson(res, 200, { cluster: id, points: history.get(id) || [] });
  }
  if (pathname.startsWith('/api/grpc/')) {
    const rest = pathname.slice('/api/grpc/'.length).split('/');
    if (rest.length !== 2) return sendJson(res, 404, { error: 'usage: /api/grpc/{cluster}/{health|query}' });
    const [id, method2] = rest;
    const cluster = byId.get(id);
    if (!cluster) return sendJson(res, 404, { error: 'no such cluster' });
    if (method2 !== 'health' && method2 !== 'query') return sendJson(res, 404, { error: 'no such grpc method' });
    let payload = {};
    if (method2 === 'query') { payload = JSON.parse((await readBody(req)).toString('utf8') || '{}'); }
    try {
      const out = await grpcCall(cluster, method2, payload);
      return sendJson(res, 200, out);
    } catch (err) {
      return sendJson(res, err.status || 502, { error: String(err.message || err) });
    }
  }
  const m = pathname.match(/^\/api\/(gw|mg)\/(.+)$/);
  if (m) {
    const [, kind, rest] = m;
    const parts = rest.split('/');
    let cluster, path;
    if (byId.has(parts[0])) { cluster = byId.get(parts[0]); path = parts.slice(1).join('/'); }
    else { cluster = defaultCluster; path = rest; }
    const routes = kind === 'gw' ? GW_ROUTES : MG_ROUTES;
    const allowed = routes.get(path);
    if (!allowed || !allowed.includes(method)) return sendJson(res, 404, { error: 'no such route' });
    return await proxy(req, res, cluster, path, method);
  }
  if (pathname.startsWith('/api/')) return sendJson(res, 404, { error: 'no such route' });
  return serveStatic(req, res, pathname);
}

const handler = (req, res) => route(req, res).catch(err => sendJson(res, err.status || 500, { error: String(err.message || err) }));

let server;
if (cfg.tlsCert && cfg.tlsKey) {
  server = createHttpsServer({ cert: readFileSync(cfg.tlsCert), key: readFileSync(cfg.tlsKey) }, handler);
} else {
  server = createHttpServer(handler);
}
const [host, port] = cfg.bind.lastIndexOf(':') > -1 ? cfg.bind.split(':') : ['127.0.0.1', cfg.bind];
server.listen(Number(port), host, () => {
  const scheme = cfg.tlsCert ? 'https' : 'http';
  console.log(`[ontolith-console] listening on ${scheme}://${host}:${port}`);
  console.log(`[ontolith-console] clusters=${clusters.map(c => c.id).join(',')} refresh=${cfg.refreshMs}ms auth=${cfg.authToken ? 'token' : 'off'} tls=${cfg.tlsCert ? 'on' : 'off'}`);
});
