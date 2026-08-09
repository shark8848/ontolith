'use strict';

const $ = (sel, root = document) => root.querySelector(sel);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};

// ---------- auth token (console access token) ----------
let consoleToken = localStorage.getItem('consoleToken') || '';
function authHeaders(extra = {}) {
  if (consoleToken) extra['Authorization'] = 'Bearer ' + consoleToken;
  return extra;
}
async function api(path, opts = {}) {
  const res = await fetch(path, { ...opts, headers: authHeaders(opts.headers || {}) });
  const text = await res.text();
  let body;
  try { body = text ? JSON.parse(text) : null; } catch { body = text; }
  if (res.status === 401 && !opts._noAuthUi) {
    showLogin();
    throw new Error('unauthorized');
  }
  if (!res.ok) {
    const msg = (body && (body.error || body.message)) || `HTTP ${res.status}`;
    throw new Error(msg);
  }
  return body;
}

// ---------- cluster state ----------
let clusters = [];
let current = null;
const gw = (p, o) => api(`/api/gw/${current.id}/${p}`, o);
const mg = (p, o) => api(`/api/mg/${current.id}/${p}`, o);

let refreshMs = 5000;
let activeTab = 'overview';
let autoTimer = null;

// ---------- login ----------
function showLogin() {
  const ov = $('#login-overlay');
  ov.classList.remove('hidden');
  $('#login-token').focus();
}
function hideLogin() { $('#login-overlay').classList.add('hidden'); }
$('#login-btn').addEventListener('click', () => {
  const t = $('#login-token').value.trim();
  if (!t) { $('#login-msg').textContent = '请输入令牌'; return; }
  consoleToken = t;
  localStorage.setItem('consoleToken', t);
  $('#login-msg').textContent = '';
  hideLogin();
  render();
});
$('#login-token').addEventListener('keydown', e => { if (e.key === 'Enter') $('#login-btn').click(); });

// ---------- clusters ----------
async function initClusters() {
  try {
    clusters = await api('/api/clusters', { _noAuthUi: true });
  } catch { clusters = [{ id: 'default', name: 'default' }]; }
  const sel = $('#cluster-select');
  sel.replaceChildren();
  for (const c of clusters) {
    const opt = el('option', null, c.name || c.id);
    opt.value = c.id;
    sel.append(opt);
  }
  const saved = localStorage.getItem('consoleCluster');
  current = clusters.find(c => c.id === saved) || clusters[0];
  sel.value = current.id;
  sel.addEventListener('change', () => {
    current = clusters.find(c => c.id === sel.value) || clusters[0];
    localStorage.setItem('consoleCluster', current.id);
    switchTab(activeTab);
  });
}

// ---------- tabs ----------
document.querySelectorAll('nav button').forEach(btn => {
  btn.addEventListener('click', () => switchTab(btn.dataset.tab));
});
function switchTab(name) {
  activeTab = name;
  document.querySelectorAll('nav button').forEach(b => b.classList.toggle('active', b.dataset.tab === name));
  document.querySelectorAll('.tab').forEach(t => t.classList.toggle('active', t.id === 'tab-' + name));
  stopAuto();
  render();
}
function startAuto(ms) { stopAuto(); if (ms > 0) autoTimer = setInterval(render, ms); }
function stopAuto() { if (autoTimer) { clearInterval(autoTimer); autoTimer = null; } }

// ---------- rendering ----------
async function render() {
  try {
    switch (activeTab) {
      case 'overview': await renderOverview(); break;
      case 'monitor': await renderMonitor(); break;
      case 'cluster': await renderCluster(); break;
      case 'infer': await renderInfer(); break;
      case 'plugins': await renderPlugins(); break;
      case 'data': await renderData(); break;
      case 'audit': await renderAudit(); break;
      case 'traces': await renderTraces(); break;
      case 'config': await renderConfig(); break;
    }
  } catch (err) {
    if (err.message !== 'unauthorized') {
      const sec = $('#tab-' + activeTab);
      sec.replaceChildren(el('p', 'err', '加载失败: ' + err.message));
    }
  }
  if (['overview', 'monitor', 'cluster', 'data'].includes(activeTab)) startAuto(refreshMs);
}

async function probeStatus() {
  if (!current) return;
  let health;
  try { health = await api(`/api/health?cluster=${current.id}`); } catch { return; }
  if (health.refresh_ms) refreshMs = health.refresh_ms;
  const gwOk = health.gateway && health.gateway.status === 'ok';
  const mgOk = health.management && health.management.status === 'ok';
  setDot('dot-gw', gwOk ? 'ok' : 'bad');
  setDot('dot-mg', mgOk ? 'ok' : 'bad');
  const note = $('#refresh-note');
  note.textContent = gwOk && mgOk
    ? `${current.name} 可达 · 自动刷新 ${refreshMs}ms`
    : `${current.name} 部分组件不可达`;
}
function setDot(id, state) { $('#' + id).className = 'dot ' + state; }

function kvCard(title, entries) {
  const card = el('div', 'card');
  card.append(el('h3', null, title));
  const dl = el('dl', 'kv');
  for (const [k, v] of entries) {
    dl.append(el('dt', null, k));
    dl.append(el('dd', null, v === undefined || v === null ? '—' : String(v)));
  }
  card.append(dl);
  return card;
}
function fmtTs(ts) {
  if (!ts) return '—';
  const d = new Date(Number(ts));
  return isNaN(d) ? String(ts) : d.toLocaleString();
}
function fmtTime(ts) {
  const d = new Date(Number(ts));
  return isNaN(d) ? '' : d.toLocaleTimeString();
}

// ---------- overview ----------
async function renderOverview() {
  const sec = $('#tab-overview');
  sec.replaceChildren();
  const [gwHealth, mgHealth, mon, metrics] = await Promise.all([
    gw('health').catch(e => ({ error: e.message })),
    mg('admin/health').catch(e => ({ error: e.message })),
    mg('admin/monitoring').catch(e => ({ error: e.message })),
    api(`/api/gw/${current.id}/metrics`).catch(() => ''),
  ]);
  const cards = el('div', 'cards');
  cards.append(kvCard('Gateway', [
    ['状态', gwHealth.error ? '不可达' : (gwHealth.status || '—')],
    ['后端', gwHealth.backend || '—'], ['三元组', gwHealth.triples ?? '—'],
    ['四元组', gwHealth.quads ?? '—'], ['鉴权', gwHealth.auth_mode || '—'],
    ['tracing', gwHealth.tracing || '—'], ['数据目录', gwHealth.data_dir || '—'],
  ]));
  cards.append(kvCard('管理面', [
    ['状态', mgHealth.error ? '不可达' : (mgHealth.status || '—')],
    ['运行时长', mgHealth.uptime_ms ? (mgHealth.uptime_ms / 1000).toFixed(1) + 's' : '—'],
    ['runtime probe', mgHealth.runtime_probe?.reachable ? '可达 ' + (mgHealth.runtime_probe.latency_ms ?? 0) + 'ms' : '不可达'],
    ['jwt / oidc', `${mgHealth.jwt || 'off'} / ${mgHealth.oidc || 'off'}`],
  ]));
  cards.append(kvCard('监控摘要', [
    ['请求总数', mon.requests_total ?? '—'], ['SPARQL 请求', mon.sparql_total ?? '—'],
    ['SPARQL 错误', mon.sparql_errors ?? '—'], ['ingest 总数', mon.ingest_total ?? '—'],
    ['平均延迟', mon.latency_avg_ms !== undefined ? mon.latency_avg_ms + 'ms' : '—'],
    ['集群节点', mon.cluster ? `${mon.cluster.healthy}/${mon.cluster.nodes}` : '—'],
  ]));
  sec.append(cards);
  const m = el('div', 'card');
  m.append(el('h3', null, 'Prometheus 指标（网关）'));
  const lines = String(metrics || '').split('\n').filter(l => l && !l.startsWith('#'));
  const tbl = el('table');
  const thead = el('thead'); const hr = el('tr');
  ['指标', '值', '时间戳'].forEach(h => hr.append(el('th', null, h)));
  thead.append(hr); tbl.append(thead);
  const tbody = el('tbody');
  for (const line of lines.slice(0, 40)) {
    const parts = line.split(/\s+/);
    if (parts.length < 2) continue;
    const tr = el('tr');
    tr.append(el('td', null, parts[0]), el('td', null, parts[1]), el('td', null, parts[2] ? fmtTs(parts[2]) : '—'));
    tbody.append(tr);
  }
  tbl.append(tbody); m.append(tbl); sec.append(m);
}

// ---------- monitor (charts) ----------
let chartCache = {};
async function renderMonitor() {
  const sec = $('#tab-monitor');
  sec.replaceChildren();
  const res = await api(`/api/history?cluster=${current.id}`);
  const pts = res.points || [];
  if (pts.length < 2) { sec.append(el('p', 'muted', '历史采样中（至少需要 2 个采样点）…')); return; }
  const series = (key) => pts.map((p, i) => ({ t: p.ts, v: p[key] ?? 0 }));
  const charts = el('div', 'charts');
  charts.append(chartCard('请求速率（req/refresh 窗口）', rateSeries(series('requests_total'))));
  charts.append(chartCard('SPARQL 请求 vs 错误', twoSeries(series('sparql_total'), series('sparql_errors'), '#4da3ff', '#e74c3c')));
  charts.append(chartCard('平均延迟（ms）', series('latency_avg_ms')));
  charts.append(chartCard('三元组总量', series('triples'), '#2ecc71'));
  charts.append(chartCard('commit_index', series('commit_index')));
  charts.append(chartCard('节点健康（healthy/nodes）', pts.map((p, i) => ({ t: p.ts, v: p.healthy ?? 0, max: p.nodes ?? 1 })), '#f1c40f'));
  sec.append(charts);
}
function rateSeries(s) {
  return s.map((p, i) => i === 0 ? { t: p.t, v: 0 } : { t: p.t, v: Math.max(0, p.v - s[i - 1].v) });
}
function twoSeries(a, b, colorA, colorB) {
  return [{ points: a, color: colorA }, { points: b, color: colorB }];
}
function chartCard(title, series, color = '#4da3ff') {
  const card = el('div', 'chart-card');
  card.append(el('h3', null, title));
  const cv = el('canvas', 'chart');
  card.append(cv);
  setTimeout(() => drawChart(cv, series, color), 0);
  return card;
}
function drawChart(cv, series, color = '#4da3ff') {
  const list = series.length > 0 && Array.isArray(series[0].points) ? series : [{ points: series, color }];
  const all = list.flatMap(s => s.points);
  if (all.length < 2) return;
  const dpr = window.devicePixelRatio || 1;
  const w = cv.clientWidth || 320, h = 140;
  cv.width = w * dpr; cv.height = h * dpr;
  const ctx = cv.getContext('2d');
  ctx.scale(dpr, dpr);
  ctx.clearRect(0, 0, w, h);
  const max = Math.max(...all.map(p => p.max ?? p.v), 1);
  const pad = 4;
  const step = (w - pad * 2) / (all.length - 1);
  const y = (v) => h - pad - (v / max) * (h - pad * 2);
  for (const s of list) {
    const pts = s.points;
    if (pts.length < 2) continue;
    ctx.strokeStyle = s.color; ctx.lineWidth = 1.5; ctx.beginPath();
    pts.forEach((p, i) => {
      const x = pad + i * step;
      i === 0 ? ctx.moveTo(x, y(p.v)) : ctx.lineTo(x, y(p.v));
    });
    ctx.stroke();
  }
  ctx.font = '10px ui-monospace, monospace';
  ctx.fillStyle = '#8b98a9';
  if (all[0]) ctx.fillText(fmtTime(all[0].t), pad, h - 2);
  let labelX = w - pad;
  for (const s of list) {
    const pts = s.points;
    if (pts.length < 2) continue;
    const last = pts[pts.length - 1];
    const text = String(last.v) + (last.max ? '/' + last.max : '');
    ctx.fillStyle = s.color;
    labelX -= ctx.measureText(text).width;
    ctx.fillText(text, labelX, h - 2);
    labelX -= 8;
  }
}

// ---------- cluster ----------
async function renderCluster() {
  const sec = $('#tab-cluster');
  sec.replaceChildren();
  const [st, mon] = await Promise.all([
    gw('cluster/status').catch(e => ({ error: e.message })),
    mg('admin/monitoring').catch(e => ({ error: e.message })),
  ]);
  const cards = el('div', 'cards');
  cards.append(kvCard('集群状态', st.error ? [['错误', st.error]]
    : [['epoch', st.epoch], ['leader', st.leader], ['节点', `${st.healthy}/${st.nodes}`],
       ['分片', st.shards], ['log_index', st.log_index], ['commit_index', st.commit_index],
       ['failovers', st.failovers], ['partition', st.partition]]));
  cards.append(kvCard('管理面视图', mon.cluster ? [
    ['epoch', mon.cluster.epoch], ['leader', mon.cluster.leader],
    ['节点', `${mon.cluster.healthy}/${mon.cluster.nodes}`], ['分片', mon.cluster.shards],
    ['shard_map_epoch', mon.cluster.shard_map_epoch], ['commit_index', mon.cluster.commit_index],
  ] : [['错误', mon.error || '不可用']]));
  sec.append(cards);
  const detail = el('div', 'card');
  detail.append(el('h3', null, '集群状态 JSON'));
  detail.append(el('pre', 'json', JSON.stringify(st, null, 2)));
  sec.append(detail);
}

// ---------- inference (L6 reasoner) ----------
const DEFAULT_SHACL_SHAPES = `@prefix ex: <http://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonShape a sh:NodeShape ;
  sh:targetClass ex:Person ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ] .
`;

async function renderInfer() {
  const sec = $('#tab-infer');
  sec.replaceChildren();
  const posture = await gw('inference').catch(e => ({ error: e.message }));
  if (posture.error) { sec.append(el('p', 'err', posture.error)); return; }
  const cards = el('div', 'cards');
  cards.append(kvCard('推理姿态', [
    ['mode', posture.mode],
    ['max_iterations', posture.max_iterations],
    ['max_elapsed_ms', posture.max_elapsed_ms ?? 'unlimited'],
    ['规则数', (posture.rules || []).length],
  ]));
  const rulesCard = el('div', 'card');
  rulesCard.append(el('h3', null, '规则清单（forward-chaining）'));
  rulesCard.append(el('pre', 'json', JSON.stringify(posture.rules || [], null, 2)));
  cards.append(rulesCard);
  sec.append(cards);

  const shaclCard = el('div', 'card');
  shaclCard.append(el('h3', null, 'SHACL 校验工作台（POST /validate/shacl）'));
  const ta = el('textarea');
  ta.value = localStorage.getItem('consoleShaclShapes') || DEFAULT_SHACL_SHAPES;
  shaclCard.append(ta);
  const shaclRow = el('div', 'row');
  const runBtn = el('button', 'run', '运行校验');
  const shaclMsg = el('span', 'muted');
  shaclRow.append(runBtn, shaclMsg);
  shaclCard.append(shaclRow);
  const shaclOut = el('pre', 'json');
  shaclOut.textContent = '—';
  shaclCard.append(shaclOut);
  runBtn.addEventListener('click', async () => {
    const body = ta.value.trim();
    if (!body) { shaclMsg.textContent = '请输入 Turtle shapes'; return; }
    runBtn.disabled = true; shaclMsg.textContent = '校验中…';
    try {
      const res = await gw('validate/shacl?limit=1000', { method: 'POST', headers: { 'content-type': 'text/turtle' }, body });
      localStorage.setItem('consoleShaclShapes', body);
      shaclMsg.textContent = res.conforms ? '✓ conforms' : `✗ 不通过（${res.result_count} 条违规）`;
      shaclOut.textContent = JSON.stringify({
        conforms: res.conforms, result_count: res.result_count,
        shapes: res.shapes, data: res.data, results: (res.results || []).slice(0, 20),
      }, null, 2);
    } catch (err) {
      shaclMsg.textContent = '';
      shaclOut.textContent = '加载失败: ' + err.message;
    } finally { runBtn.disabled = false; }
  });
  sec.append(shaclCard);

  const matCard = el('div', 'card');
  matCard.append(el('h3', null, '物化运行（POST /materialize）'));
  const matRow = el('div', 'row');
  const matBtn = el('button', 'run', '运行物化');
  const matMsg = el('span', 'muted');
  matRow.append(matBtn, matMsg);
  matCard.append(matRow);
  const matOut = el('pre', 'json');
  matOut.textContent = '—';
  matCard.append(matOut);
  matBtn.addEventListener('click', async () => {
    matBtn.disabled = true; matMsg.textContent = '物化中…';
    try {
      const res = await gw('materialize', { method: 'POST' });
      const warn = (res.timed_out ? ' · 超时' : '') + (res.inconsistent ? ' · inconsistent' : '');
      matMsg.textContent = `${res.derived_triples} 条派生 · ${res.elapsed_ms}ms${warn}`;
      matOut.textContent = JSON.stringify(res, null, 2);
    } catch (err) {
      matMsg.textContent = '';
      matOut.textContent = '加载失败: ' + err.message;
    } finally { matBtn.disabled = false; }
  });
  sec.append(matCard);
}

// ---------- plugins (L8 plugin-api) ----------
async function renderPlugins() {
  const sec = $('#tab-plugins');
  sec.replaceChildren();
  const res = await mg('admin/plugins').catch(e => ({ error: e.message }));
  if (res.error) { sec.append(el('p', 'err', res.error)); return; }
  const cards = el('div', 'cards');
  cards.append(kvCard('插件契约状态', [
    ['status', res.status],
    ['api_version', res.api_version],
    ['插件数', (res.plugins || []).length],
    ['能力数', (res.capabilities || []).length],
  ]));
  const capCard = el('div', 'card');
  capCard.append(el('h3', null, '能力集合（PluginCapability）'));
  const capRow = el('div', 'row');
  for (const c of res.capabilities || []) capRow.append(el('span', 'tag', c));
  capCard.append(capRow);
  cards.append(capCard);
  sec.append(cards);

  for (const p of res.plugins || []) {
    const card = el('div', 'card');
    card.append(el('h3', null, p.id));
    const dl = el('dl', 'kv');
    dl.append(el('dt', null, '版本'), el('dd', null, p.version));
    dl.append(el('dt', null, '契约 api_version'), el('dd', null, p.api_version));
    dl.append(el('dt', null, '能力'), el('dd', null, (p.capabilities || []).join('、')));
    card.append(dl);
    for (const tool of p.tools || []) {
      card.append(el('h4', null, '工具 · ' + tool.name));
      card.append(el('p', 'muted', tool.description));
      const tbl = el('table');
      const thead = el('thead');
      const hr = el('tr');
      ['参数', '说明', '必填'].forEach(h => hr.append(el('th', null, h)));
      thead.append(hr);
      tbl.append(thead);
      const tbody = el('tbody');
      for (const pa of tool.parameters || []) {
        const tr = el('tr');
        tr.append(el('td', null, pa.name), el('td', null, pa.description), el('td', null, pa.required ? '是' : '否'));
        tbody.append(tr);
      }
      tbl.append(tbody);
      card.append(tbl);
    }
    sec.append(card);
  }

  const contractCard = el('div', 'card');
  contractCard.append(el('h3', null, '契约状态（ontolith-plugin-api）'));
  contractCard.append(el('pre', 'json', JSON.stringify(res.contracts || {}, null, 2)));
  sec.append(contractCard);
}

// ---------- SPARQL (HTTP / gRPC channel) ----------
function buildSparqlTab() {
  const sec = $('#tab-sparql');
  sec.replaceChildren();
  const box = el('textarea');
  box.placeholder = 'SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 50';
  box.value = 'SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 50';
  sec.append(box);
  const row = el('div', 'row');
  const runBtn = el('button', 'run', '运行查询');
  const expBtn = el('button', 'run secondary', 'Explain');
  const ch = el('select');
  ['HTTP', 'gRPC'].forEach(m => { const o = el('option', null, m); o.value = m; ch.append(o); });
  const msg = el('span', 'muted');
  row.append(runBtn, expBtn, ch, msg);
  sec.append(row);
  const out = el('div');
  sec.append(out);

  async function execute(explain) {
    const q = box.value.trim();
    if (!q) { msg.textContent = '请输入查询'; return; }
    runBtn.disabled = expBtn.disabled = true;
    msg.textContent = '执行中…';
    try {
      out.replaceChildren();
      let res;
      if (ch.value === 'gRPC') {
        res = await api(`/api/grpc/${current.id}/query`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ query: q, explain }),
        });
        if (explain) out.append(el('pre', 'json', JSON.stringify(res, null, 2)));
        else renderSparqlResult(out, res);
        msg.textContent = 'gRPC 通道';
      } else {
        res = await api(`/api/gw/${current.id}/` + (explain ? 'explain' : 'sparql') + '?query=' + encodeURIComponent(q));
        if (explain) out.append(el('pre', 'json', typeof res === 'string' ? res : JSON.stringify(res, null, 2)));
        else renderSparqlResult(out, res);
        msg.textContent = 'HTTP 通道';
      }
    } catch (err) {
      if (err.message !== 'unauthorized') msg.textContent = '错误: ' + err.message;
    } finally {
      runBtn.disabled = expBtn.disabled = false;
    }
  }
  runBtn.addEventListener('click', () => execute(false));
  expBtn.addEventListener('click', () => execute(true));
  box.addEventListener('keydown', e => { if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') execute(false); });
}

function renderSparqlResult(out, res) {
  const head = res.head?.vars || [];
  const rows = res.results?.bindings || [];
  if (head.length === 0) { out.append(el('p', 'muted', '查询返回空结果')); return; }
  const tbl = el('table');
  const thead = el('thead'); const hr = el('tr');
  head.forEach(v => hr.append(el('th', null, v)));
  thead.append(hr); tbl.append(thead);
  const tbody = el('tbody');
  for (const b of rows) {
    const tr = el('tr');
    for (const v of head) {
      const cell = b[v];
      const td = el('td');
      if (!cell) td.textContent = '';
      else if (cell.type === 'uri') td.append(el('code', null, cell.value));
      else if (cell.type === 'literal') {
        td.append(el('span', null, cell.value));
        if (cell.datatype && cell.datatype !== 'http://www.w3.org/2001/XMLSchema#string') {
          td.append(el('span', 'tag', '^' + cell.datatype.split(/[#/]/).pop()));
        }
        if (cell['xml:lang']) td.append(el('span', 'tag', '@' + cell['xml:lang']));
      } else td.append(el('span', 'tag', cell.type), el('span', null, ' ' + (cell.value || '')));
      tr.append(td);
    }
    tbody.append(tr);
  }
  tbl.append(tbody); out.append(tbl);
  const meta = res.meta || {};
  out.append(el('p', 'muted', `${rows.length} 行 · ${meta.elapsed_ms ?? '?'}ms · tenant=${meta.tenant ?? '?'} · consistency=${meta.consistency ?? '?'}`));
}

// ---------- data ----------
async function renderData() {
  const sec = $('#tab-data');
  sec.replaceChildren();
  const stats = await mg('admin/data/stats').catch(e => ({ error: e.message }));
  const cards = el('div', 'cards');
  cards.append(kvCard('数据统计', stats.error ? [['错误', stats.error]] : [
    ['三元组', stats.triples], ['四元组', stats.quads],
    ['pending 事务', stats.pending_txns], ['审计事件', stats.audit_events],
    ['存储后端', stats.storage_backend],
  ]));
  sec.append(cards);
  const card = el('div', 'card');
  card.append(el('h3', null, 'Turtle 写入（POST /data/turtle）'));
  const ta = el('textarea');
  ta.placeholder = '@prefix ex: <http://example.org/> .\nex:demo a ex:Thing ; ex:label "demo" .';
  card.append(ta);
  const row = el('div', 'row');
  const btn = el('button', 'run', '写入');
  const msg = el('span', 'muted');
  row.append(btn, msg);
  card.append(row);
  const out = el('pre', 'json');
  card.append(out);
  btn.addEventListener('click', async () => {
    const body = ta.value.trim();
    if (!body) { msg.textContent = '请输入 Turtle'; return; }
    btn.disabled = true; msg.textContent = '写入中…';
    try {
      const res = await gw('data/turtle', { method: 'POST', headers: { 'content-type': 'text/turtle' }, body });
      out.textContent = JSON.stringify(res, null, 2);
      msg.textContent = '写入成功';
      renderData();
    } catch (err) {
      if (err.message !== 'unauthorized') msg.textContent = '错误: ' + err.message;
    } finally { btn.disabled = false; }
  });
  sec.append(card);
}

// ---------- audit ----------
async function renderAudit() {
  const sec = $('#tab-audit');
  sec.replaceChildren();
  const row = el('div', 'row');
  const btn = el('button', 'run secondary', '刷新');
  const msg = el('span', 'muted');
  row.append(btn, msg, el('span', 'spacer'));
  sec.append(row);
  const res = await mg('admin/data/audit?limit=200').catch(e => ({ error: e.message, events: [] }));
  if (res.error) { sec.append(el('p', 'err', res.error)); return; }
  msg.textContent = `共 ${res.total ?? res.events.length} 条，显示 ${res.events.length} 条`;
  const tbl = el('table');
  const thead = el('thead'); const hr = el('tr');
  ['时间', '租户', '用户', '动作', '资源', '结果', '详情'].forEach(h => hr.append(el('th', null, h)));
  thead.append(hr); tbl.append(thead);
  const tbody = el('tbody');
  for (const ev of res.events || []) {
    const tr = el('tr');
    tr.append(el('td', null, fmtTs(ev.ts)), el('td', null, ev.tenant || '—'), el('td', null, ev.user || '—'),
      el('td', null, ev.action || '—'), el('td', null, ev.resource || '—'), el('td', null, ev.outcome || '—'),
      el('td', null, ev.detail || '—'));
    tbody.append(tr);
  }
  tbl.append(tbody); sec.append(tbl);
  btn.addEventListener('click', renderAudit);
}

// ---------- traces ----------
async function renderTraces() {
  const sec = $('#tab-traces');
  sec.replaceChildren();
  const row = el('div', 'row');
  const btn = el('button', 'run secondary', '刷新');
  const msg = el('span', 'muted');
  row.append(btn, msg);
  sec.append(row);
  const res = await mg('admin/traces?limit=100').catch(e => ({ error: e.message, traces: [] }));
  if (res.error) { sec.append(el('p', 'err', res.error)); return; }
  msg.textContent = `共 ${res.total ?? res.traces.length} 条 trace`;
  const list = el('div', 'cards');
  for (const t of res.traces || []) {
    const card = el('div', 'card');
    card.append(el('h3', null, (t.trace_id || '').slice(0, 18) + '…'));
    card.append(el('pre', 'json', JSON.stringify(t, null, 2)));
    list.append(card);
  }
  sec.append(list);
  btn.addEventListener('click', renderTraces);
}

// ---------- config ----------
async function renderConfig() {
  const sec = $('#tab-config');
  sec.replaceChildren();
  const res = await mg('admin/config').catch(e => ({ error: e.message }));
  const card = el('div', 'card');
  card.append(el('h3', null, '管理面配置（密钥已脱敏）'));
  card.append(el('pre', 'json', JSON.stringify(res, null, 2)));
  sec.append(card);
  const layers = await mg('admin/layers').catch(() => null);
  if (layers) {
    const lc = el('div', 'card');
    lc.append(el('h3', null, `架构分层（${layers.layer_count}）`));
    const tbl = el('table'); const thead = el('thead'); const hr = el('tr');
    ['层', 'crate', '域'].forEach(h => hr.append(el('th', null, h)));
    thead.append(hr); tbl.append(thead);
    const tbody = el('tbody');
    for (const l of layers.layers || []) {
      const tr = el('tr');
      tr.append(el('td', null, l.id), el('td', null, l.crate), el('td', null, l.domain));
      tbody.append(tr);
    }
    tbl.append(tbody); lc.append(tbl); sec.append(lc);
  }
}

// ---------- boot ----------
(async () => {
  await initClusters();
  buildSparqlTab();
  setInterval(probeStatus, refreshMs > 0 ? refreshMs : 5000);
  probeStatus();
  render();
})();
