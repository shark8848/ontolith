// Zero-dependency gRPC client for the ontolith SparqlService (node:http2 +
// hand-rolled protobuf wire codec). Service: ontolith.v1.SparqlService.
import { connect } from 'node:http2';

// ---- protobuf wire helpers ----
function varint(n) {
  n = Number(n) >>> 0;
  if (!Number.isSafeInteger(n)) n = BigInt(n);
  if (typeof n === 'bigint') {
    const out = [];
    let v = n;
    while (v > 0x7fn) { out.push(Number(v & 0x7fn) | 0x80); v >>= 7n; }
    out.push(Number(v));
    return Buffer.from(out);
  }
  const out = [];
  while (n > 0x7f) { out.push((n & 0x7f) | 0x80); n >>>= 7; }
  out.push(n);
  return Buffer.from(out);
}
const tag = (num, wt) => varint((num << 3) | wt);
const fVarint = (num, val) => Buffer.concat([tag(num, 0), varint(val)]);
const fString = (num, str) => {
  const b = Buffer.from(str, 'utf8');
  return Buffer.concat([tag(num, 2), varint(b.length), b]);
};

// QueryRequest: 1 query, 2 format, 3 explain, 4 timeout_ms, 5 consistency
function encodeQuery({ query, format, explain, timeoutMs, consistency }) {
  const parts = [];
  if (query) parts.push(fString(1, query));
  if (format) parts.push(fString(2, format));
  if (explain) parts.push(fVarint(3, 1));
  if (timeoutMs) parts.push(fVarint(4, timeoutMs));
  if (consistency) parts.push(fString(5, consistency));
  return Buffer.concat(parts);
}
// HealthRequest: empty
const encodeHealth = () => Buffer.alloc(0);

// QueryResponse: 1 ok, 2 http_status, 3 body, 4 error
function decodeQuery(buf) {
  const out = { ok: false, http_status: 0, body: '', error: '' };
  let i = 0;
  while (i < buf.length) {
    const t = readVarint(buf, i); i = t.n;
    const field = t.v >> 3, wt = t.v & 7;
    if (wt === 0) { const r = readVarint(buf, i); i = r.n; if (field === 1) out.ok = r.v !== 0; else if (field === 2) out.http_status = r.v; }
    else if (wt === 2) { const r = readVarint(buf, i); i = r.n; const end = i + r.v; if (field === 3) out.body = buf.subarray(i, end).toString('utf8'); else if (field === 4) out.error = buf.subarray(i, end).toString('utf8'); i = end; }
    else throw new Error('unsupported wire type ' + wt);
  }
  return out;
}
// HealthResponse: all strings (1 status, 2 backend, 3 tenant_mode, 4 auth_mode, 5 jwt, 6 tracing, 7 oidc)
function decodeHealth(buf) {
  const out = {};
  let i = 0;
  while (i < buf.length) {
    const t = readVarint(buf, i); i = t.n;
    const field = t.v >> 3, wt = t.v & 7;
    if (wt !== 2) throw new Error('unsupported wire type ' + wt);
    const r = readVarint(buf, i); i = r.n;
    const end = i + r.v;
    const s = buf.subarray(i, end).toString('utf8');
    const names = { 1: 'status', 2: 'backend', 3: 'tenant_mode', 4: 'auth_mode', 5: 'jwt', 6: 'tracing', 7: 'oidc' };
    if (names[field]) out[names[field]] = s;
    i = end;
  }
  return out;
}
function readVarint(buf, i) {
  let v = 0, shift = 0;
  for (;;) {
    const b = buf[i++];
    v |= (b & 0x7f) << shift;
    if (!(b & 0x80)) break;
    shift += 7;
  }
  return { v: v >>> 0, n: i };
}

// ---- gRPC framing + transport ----
const frame = (msg) => {
  const head = Buffer.alloc(5);
  head[0] = 0; // no compression
  head.writeUInt32BE(msg.length, 1);
  return Buffer.concat([head, msg]);
};

export function callGrpc({ grpcUrl, path, message, metadata = {}, timeoutMs = 15000 }) {
  return new Promise((resolve, reject) => {
    const session = connect(grpcUrl, { createConnection: undefined });
    const timer = setTimeout(() => { session.destroy(); reject(new Error('gRPC timeout')); }, timeoutMs);
    const headers = {
      ':method': 'POST',
      ':scheme': 'http',
      ':authority': grpcUrl.replace(/^https?:\/\//, ''),
      ':path': path,
      'content-type': 'application/grpc',
      'te': 'trailers',
      ...Object.fromEntries(Object.entries(metadata).map(([k, v]) => [k.toLowerCase(), String(v)])),
    };
    const req = session.request(headers);
    let chunks = [];
    let grpcStatus = null, grpcMessage = '';
    req.on('response', () => {});
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('trailers', (tr) => { grpcStatus = tr['grpc-status']; grpcMessage = tr['grpc-message'] || ''; });
    req.on('error', (err) => { clearTimeout(timer); session.destroy(); reject(err); });
    req.on('end', () => {
      clearTimeout(timer);
      session.close();
      if (grpcStatus && grpcStatus !== '0') return reject(new Error(`gRPC status ${grpcStatus}: ${grpcMessage || 'error'}`));
      let body = Buffer.concat(chunks);
      // strip 5-byte frame header(s); gRPC server sends one message frame per response
      const out = [];
      while (body.length >= 5) {
        const len = body.readUInt32BE(1);
        out.push(body.subarray(5, 5 + len));
        body = body.subarray(5 + len);
      }
      resolve(Buffer.concat(out));
    });
    req.end(frame(message));
  });
}

export const PATHS = {
  health: '/ontolith.v1.SparqlService/Health',
  query: '/ontolith.v1.SparqlService/Query',
};
export const encode = { query: encodeQuery, health: encodeHealth };
export const decode = { query: decodeQuery, health: decodeHealth };
