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
// Page-local auto-refresh timers — only monitor / cluster self-refresh.
let monitorTimer = null;
let clusterTimer = null;
// Transient outputs preserved across re-renders (manual refresh / tab switch).
let lastTenantKey = null; // { tenant, apiKey } — one-time key shown inside the tenant card.
let lastShacl = null;
let lastMat = null;
let lastTurtle = null;

// ---------- themes / UI settings (bottom-left config menu) ----------
const THEMES = [
  { id: 'midnight', name: '午夜蓝', desc: '深邃蓝黑 · 默认', swatch: ['#0f1419', '#171d26', '#4da3ff', '#2ecc71'] },
  { id: 'graphite', name: '石墨', desc: '冷灰质感', swatch: ['#111418', '#191d22', '#58a6ff', '#2ecc71'] },
  { id: 'forest', name: '森林', desc: '墨绿静谧', swatch: ['#0c1512', '#13211b', '#43c98a', '#2ecc71'] },
  { id: 'dusk', name: '暮紫', desc: '蓝紫暮色', swatch: ['#110f1b', '#181526', '#9d7bff', '#2ecc71'] },
  { id: 'paper', name: '纸白', desc: '浅色明亮', swatch: ['#f2f5f9', '#ffffff', '#4d94ff', '#1f9d57'] },
];
function cssVar(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#4da3ff';
}
function applyTheme(id) {
  document.documentElement.dataset.theme = id;
  localStorage.setItem('consoleTheme', id);
  document.querySelectorAll('.theme-card').forEach(c => c.classList.toggle('active', c.dataset.theme === id));
}
function buildThemeGrid() {
  const grid = $('#theme-grid');
  grid.replaceChildren();
  for (const t of THEMES) {
    const card = el('button', 'theme-card' + (t.id === (localStorage.getItem('consoleTheme') || 'midnight') ? ' active' : ''));
    card.dataset.theme = t.id;
    const sw = el('span', 'theme-swatch');
    for (const c of t.swatch) {
      const chip = document.createElement('span');
      chip.style.background = c;
      sw.append(chip);
    }
    card.append(sw);
    const name = el('b', null, t.name);
    card.append(name, el('small', null, t.desc));
    card.addEventListener('click', () => applyTheme(t.id));
    grid.append(card);
  }
}
function openSettings() { buildThemeGrid(); $('#settings-overlay').classList.remove('hidden'); }
function closeSettings() { $('#settings-overlay').classList.add('hidden'); }
function logout() {
  stopPageAuto();
  consoleToken = '';
  localStorage.removeItem('consoleToken');
  closeSettings();
  showLogin();
}

// ---------- login ----------
function showLogin() {
  closeSettings();
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
  stopPageAuto();
  render();
}
function stopPageAuto() {
  clearInterval(monitorTimer);
  clearInterval(clusterTimer);
}
function editingInTab() {
  const ae = document.activeElement;
  return !!ae && !!ae.closest && !!ae.closest('.tab.active') &&
    ['INPUT', 'TEXTAREA', 'SELECT'].includes(ae.tagName);
}

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
      case 'tenant': await renderTenant(); break;
      case 'traces': await renderTraces(); break;
      case 'config': await renderConfig(); break;
    }
  } catch (err) {
    if (err.message !== 'unauthorized') {
      const sec = $('#tab-' + activeTab);
      sec.replaceChildren(el('p', 'err', '加载失败: ' + err.message));
    }
  }
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
    ? `${current.name} 可达 · 监控/集群自动刷新 ${refreshMs}ms`
    : `${current.name} 部分组件不可达`;
}
function setDot(id, state) { $('#' + id).className = 'dot ' + state; }
function fmtLatency(ms) {
  if (ms === undefined || ms === null || isNaN(Number(ms))) return '—';
  return Number(ms).toFixed(3);
}
function fmtNum(v) {
  if (v === undefined || v === null || v === '') return '—';
  const n = Number(v);
  if (isNaN(n) || !isFinite(n)) return String(v);
  return Number.isInteger(n) ? String(n) : n.toFixed(3);
}
function iconBtn(symbolId, title) {
  const b = el('button', 'icon-btn');
  b.type = 'button';
  b.title = title;
  b.setAttribute('aria-label', title);
  b.innerHTML = `<svg class="icon"><use href="#${symbolId}"></use></svg>`;
  return b;
}
const TOAST_MS = 5000;
function toast(msg, type = 'ok') {
  const box = $('#toast-container');
  if (!box) return;
  const t = el('div', 'toast' + (type === 'bad' ? ' bad' : ''));
  t.append(el('div', 'toast-msg', msg));
  t.append(el('div', 'toast-bar'));
  box.append(t);
  setTimeout(() => { t.classList.add('out'); setTimeout(() => t.remove(), 350); }, TOAST_MS);
}
function confirmDeleteTenant(t) {
  return new Promise(resolve => {
    const ov = el('div', 'confirm-overlay');
    const card = el('div', 'confirm-card');
    card.append(el('h2', null, '删除租户'));
    card.append(el('p', 'muted', `确认删除租户 ${t.id}？其 API key 将立即失效。请输入租户 id 以确认：`));
    const input = el('input'); input.type = 'text'; input.placeholder = t.id;
    const err = el('p', 'err');
    const row = el('div', 'row');
    const cancel = el('button', 'run secondary', '取消');
    const confirm = el('button', 'run', '确认删除');
    confirm.disabled = true;
    const close = (ok) => { ov.remove(); resolve(ok); };
    input.addEventListener('input', () => { confirm.disabled = input.value !== t.id; err.textContent = ''; });
    input.addEventListener('keydown', e => {
      if (e.key === 'Enter' && !confirm.disabled) confirm.click();
      if (e.key === 'Escape') close(false);
    });
    cancel.addEventListener('click', () => close(false));
    confirm.addEventListener('click', () => {
      if (input.value !== t.id) { err.textContent = '输入与租户 id 不匹配'; return; }
      close(true);
    });
    ov.addEventListener('click', e => { if (e.target === ov) close(false); });
    row.append(cancel, confirm);
    card.append(input, err, row);
    ov.append(card);
    document.body.append(ov);
    input.focus();
  });
}
async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = el('textarea');
    ta.value = text;
    ta.style.cssText = 'position:fixed;top:0;left:0;opacity:0;pointer-events:none';
    document.body.append(ta);
    ta.select();
    let ok = false;
    try { ok = document.execCommand('copy'); } catch { /* noop */ }
    ta.remove();
    return ok;
  }
}

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
    ['运行时长', mgHealth.uptime_ms ? fmtNum(mgHealth.uptime_ms / 1000) + 's' : '—'],
    ['runtime probe', mgHealth.runtime_probe?.reachable ? '可达 ' + fmtLatency(mgHealth.runtime_probe.latency_ms ?? 0) + 'ms' : '不可达'],
    ['jwt / oidc', `${mgHealth.jwt || 'off'} / ${mgHealth.oidc || 'off'}`],
  ]));
  cards.append(kvCard('监控摘要', [
    ['请求总数', fmtNum(mon.requests_total)], ['SPARQL 请求', fmtNum(mon.sparql_total)],
    ['SPARQL 错误', fmtNum(mon.sparql_errors)], ['ingest 总数', fmtNum(mon.ingest_total)],
    ['平均延迟', mon.latency_avg_ms !== undefined ? fmtLatency(mon.latency_avg_ms) + 'ms' : '—'],
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
    tr.append(el('td', null, parts[0]), el('td', null, fmtNum(parts[1])), el('td', null, parts[2] ? fmtTs(parts[2]) : '—'));
    tbody.append(tr);
  }
  tbl.append(tbody); m.append(tbl); sec.append(m);
}

// ---------- monitor (charts) ----------
let chartCache = {};
async function renderMonitor() {
  clearInterval(monitorTimer);
  const schedule = () => { monitorTimer = setInterval(() => { if (!editingInTab()) renderMonitor(); }, refreshMs); };
  const sec = $('#tab-monitor');
  sec.replaceChildren();
  const res = await api(`/api/history?cluster=${current.id}`);
  const pts = res.points || [];
  if (pts.length < 2) { sec.append(el('p', 'muted', '历史采样中（至少需要 2 个采样点）…')); schedule(); return; }
  const series = (key) => pts.map((p, i) => ({ t: p.ts, v: p[key] ?? 0 }));
  const charts = el('div', 'charts');
  charts.append(chartCard('请求速率（req/refresh 窗口）', rateSeries(series('requests_total'))));
  charts.append(chartCard('SPARQL 请求 vs 错误', twoSeries(series('sparql_total'), series('sparql_errors'), cssVar('--accent'), cssVar('--bad'))));
  charts.append(chartCard('平均延迟（ms）', series('latency_avg_ms'), undefined, fmtLatency));
  charts.append(chartCard('三元组总量', series('triples'), cssVar('--ok')));
  charts.append(chartCard('commit_index', series('commit_index')));
  charts.append(chartCard('节点健康（healthy/nodes）', pts.map((p, i) => ({ t: p.ts, v: p.healthy ?? 0, max: p.nodes ?? 1 })), cssVar('--warn')));
  sec.append(charts);
  schedule();
}
function rateSeries(s) {
  return s.map((p, i) => i === 0 ? { t: p.t, v: 0 } : { t: p.t, v: Math.max(0, p.v - s[i - 1].v) });
}
function twoSeries(a, b, colorA, colorB) {
  return [{ points: a, color: colorA }, { points: b, color: colorB }];
}
function chartCard(title, series, color = cssVar('--accent'), fmt) {
  const card = el('div', 'chart-card');
  card.append(el('h3', null, title));
  const cv = el('canvas', 'chart');
  card.append(cv);
  setTimeout(() => drawChart(cv, series, color, fmt), 0);
  return card;
}
function drawChart(cv, series, color = cssVar('--accent'), fmt) {
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
  ctx.fillStyle = cssVar('--muted');
  if (all[0]) ctx.fillText(fmtTime(all[0].t), pad, h - 2);
  let labelX = w - pad;
  for (const s of list) {
    const pts = s.points;
    if (pts.length < 2) continue;
    const last = pts[pts.length - 1];
    const text = (fmt ? fmt(last.v) : fmtNum(last.v)) + (last.max ? '/' + last.max : '');
    ctx.fillStyle = s.color;
    labelX -= ctx.measureText(text).width;
    ctx.fillText(text, labelX, h - 2);
    labelX -= 8;
  }
}

// ---------- cluster ----------
async function renderCluster() {
  clearInterval(clusterTimer);
  const schedule = () => { clusterTimer = setInterval(() => { if (!editingInTab()) renderCluster(); }, refreshMs); };
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
  schedule();
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
  ta.addEventListener('input', () => localStorage.setItem('consoleShaclShapes', ta.value));
  shaclCard.append(ta);
  const shaclRow = el('div', 'row');
  const runBtn = el('button', 'run', '运行校验');
  const shaclMsg = el('span', 'muted');
  shaclRow.append(runBtn, shaclMsg);
  shaclCard.append(shaclRow);
  const shaclOut = el('pre', 'json');
  shaclOut.textContent = '—';
  shaclCard.append(shaclOut);
  if (lastShacl) { shaclMsg.textContent = lastShacl.msg; shaclOut.textContent = lastShacl.out; }
  runBtn.addEventListener('click', async () => {
    const body = ta.value.trim();
    if (!body) { shaclMsg.textContent = '请输入 Turtle shapes'; return; }
    runBtn.disabled = true; shaclMsg.textContent = '校验中…';
    try {
      const res = await gw('validate/shacl?limit=1000', { method: 'POST', headers: { 'content-type': 'text/turtle' }, body });
      localStorage.setItem('consoleShaclShapes', body);
      const msg = res.conforms ? '✓ conforms' : `✗ 不通过（${res.result_count} 条违规）`;
      const out = JSON.stringify({
        conforms: res.conforms, result_count: res.result_count,
        shapes: res.shapes, data: res.data, results: (res.results || []).slice(0, 20),
      }, null, 2);
      shaclMsg.textContent = msg;
      shaclOut.textContent = out;
      lastShacl = { msg, out };
    } catch (err) {
      shaclMsg.textContent = '';
      shaclOut.textContent = '加载失败: ' + err.message;
      lastShacl = { msg: '', out: '加载失败: ' + err.message };
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
  if (lastMat) { matMsg.textContent = lastMat.msg; matOut.textContent = lastMat.out; }
  matBtn.addEventListener('click', async () => {
    matBtn.disabled = true; matMsg.textContent = '物化中…';
    try {
      const res = await gw('materialize', { method: 'POST' });
      const warn = (res.timed_out ? ' · 超时' : '') + (res.inconsistent ? ' · inconsistent' : '');
      const msg = `${res.derived_triples} 条派生 · ${res.elapsed_ms}ms${warn}`;
      const out = JSON.stringify(res, null, 2);
      matMsg.textContent = msg;
      matOut.textContent = out;
      lastMat = { msg, out };
    } catch (err) {
      matMsg.textContent = '';
      matOut.textContent = '加载失败: ' + err.message;
      lastMat = { msg: '', out: '加载失败: ' + err.message };
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
  ta.value = localStorage.getItem('consoleTurtleDraft') || '';
  ta.addEventListener('input', () => localStorage.setItem('consoleTurtleDraft', ta.value));
  card.append(ta);
  const row = el('div', 'row');
  const btn = el('button', 'run', '写入');
  const msg = el('span', 'muted');
  row.append(btn, msg);
  card.append(row);
  const out = el('pre', 'json');
  card.append(out);
  if (lastTurtle) { msg.textContent = lastTurtle.msg; out.textContent = lastTurtle.out; }
  btn.addEventListener('click', async () => {
    const body = ta.value.trim();
    if (!body) { msg.textContent = '请输入 Turtle'; return; }
    btn.disabled = true; msg.textContent = '写入中…';
    try {
      const res = await gw('data/turtle', { method: 'POST', headers: { 'content-type': 'text/turtle' }, body });
      const outText = JSON.stringify(res, null, 2);
      out.textContent = outText;
      msg.textContent = '写入成功';
      lastTurtle = { msg: '写入成功', out: outText };
      renderData();
    } catch (err) {
      if (err.message !== 'unauthorized') msg.textContent = '错误: ' + err.message;
      lastTurtle = { msg: '错误: ' + err.message, out: out.textContent };
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


// ---------- tenants ----------
async function renderTenant() {
  const sec = $('#tab-tenant');
  sec.replaceChildren();
  const list = el('div', 'cards');

  // Create form (always visible at the top).
  const formCard = el('div', 'card');
  formCard.append(el('h3', null, '创建租户'));
  const form = el('div', 'row');
  const idInput = el('input'); idInput.type = 'text'; idInput.placeholder = 'id（[a-z0-9_-]，≤64）';
  const nameInput = el('input'); nameInput.type = 'text'; nameInput.placeholder = '名称';
  const descInput = el('input'); descInput.type = 'text'; descInput.placeholder = '描述';
  const genBox = el('label'); genBox.style.cssText = 'display:flex;gap:6px;align-items:center;font-size:13px;color:var(--muted)';
  const genCb = document.createElement('input'); genCb.type = 'checkbox'; genCb.checked = true;
  genBox.append(genCb, document.createTextNode('生成 API key'));
  const createBtn = el('button', 'run', '创建');
  const formMsg = el('span', 'muted');
  form.append(idInput, nameInput, descInput, genBox, createBtn, formMsg);
  formCard.append(form);
  list.append(formCard);

  createBtn.addEventListener('click', async () => {
    const id = idInput.value.trim();
    if (!id) { formMsg.textContent = '请输入租户 id'; return; }
    formMsg.textContent = '创建中…';
    try {
      const out = await mg('admin/tenants', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          id,
          name: nameInput.value.trim(),
          description: descInput.value.trim(),
          status: 'active',
          generate_key: genCb.checked,
        }),
      });
      idInput.value = nameInput.value = descInput.value = '';
      const t = out.tenant || {};
      if (out.api_key) lastTenantKey = { tenant: t.id, apiKey: out.api_key };
      formMsg.textContent = '';
      toast(`已创建 ${t.id}`);
      await renderTenant();
    } catch (e) { formMsg.textContent = '创建失败: ' + e.message; }
  });

  const res = await mg('admin/tenants').catch(e => ({ error: e.message }));
  if (res.error) { sec.append(el('p', 'err', '加载失败: ' + res.error)); return; }

  for (const t of res.tenants || []) {
    const card = el('div', 'card');
    const head = el('div', 'row');
    head.style.cssText = 'justify-content:space-between;margin:0 0 6px';
    const title = el('h3', null, `${t.name || t.id} · ${t.id}`);
    title.style.cssText = 'margin:0;color:var(--text);text-transform:none;letter-spacing:0';
    head.append(title);
    head.append(el('span', 'tag ' + (t.status === 'active' ? 'ok' : 'bad'), t.status));
    card.append(head);
    card.append(el('p', 'muted', t.description || '—'));
    const dl = el('dl', 'kv');
    dl.append(el('dt', null, '创建'), el('dd', null, new Date(t.created_at_ms).toLocaleString()));
    dl.append(el('dt', null, '更新'), el('dd', null, new Date(t.updated_at_ms).toLocaleString()));
    card.append(dl);

    if (lastTenantKey && lastTenantKey.tenant === t.id) {
      const box = el('div', 'key-box');
      box.append(el('div', 'key-box-title', '一次性 API key（仅显示一次）'));
      const row = el('div', 'row');
      row.style.cssText = 'margin:0';
      const code = el('code', 'key-value', lastTenantKey.apiKey);
      const copyBtn = iconBtn('icon-copy', '复制');
      copyBtn.addEventListener('click', async () => {
        const ok = await copyText(lastTenantKey.apiKey);
        copyBtn.classList.add('copied');
        copyBtn.title = ok ? '已复制' : '复制失败';
        setTimeout(() => { copyBtn.classList.remove('copied'); copyBtn.title = '复制'; }, 1500);
      });
      row.append(code, copyBtn);
      box.append(row);
      card.append(box);
    }

    if ((t.api_keys || []).length > 0) {
      const tbl = el('table');
      const thead = el('thead'); const hr = el('tr');
      ['key', '标签', '创建', ''].forEach(h => hr.append(el('th', null, h)));
      thead.append(hr); tbl.append(thead);
      const tbody = el('tbody');
      for (const k of t.api_keys || []) {
        const tr = el('tr');
        tr.append(el('td', null, k.id), el('td', null, k.label || '—'), el('td', null, new Date(k.created_at_ms).toLocaleString()));
        const revoke = el('button', 'run secondary', '吊销');
        revoke.addEventListener('click', async () => {
          try {
            await mg(`admin/tenants/${t.id}/keys/${k.id}`, { method: 'DELETE' });
            await renderTenant();
          } catch (e) { alert('吊销失败: ' + e.message); }
        });
        const td = el('td'); td.append(revoke); tr.append(td);
        tbody.append(tr);
      }
      tbl.append(tbody);
      card.append(tbl);
    }

    // Add-key row.
    const keyRow = el('div', 'row');
    const keyInput = el('input'); keyInput.type = 'text'; keyInput.placeholder = 'key 标签（可选）';
    const addKeyBtn = el('button', 'run secondary', '生成 key');
    const keyMsg = el('span', 'muted');
    addKeyBtn.addEventListener('click', async () => {
      keyMsg.textContent = '';
      try {
        const out = await mg(`admin/tenants/${t.id}/keys`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ label: keyInput.value.trim() }),
        });
        keyInput.value = '';
        lastTenantKey = { tenant: t.id, apiKey: out.api_key };
        keyMsg.textContent = '';
        toast(`已为 ${t.id} 生成新 key`);
        await renderTenant();
      } catch (e) { keyMsg.textContent = '生成失败: ' + e.message; }
    });
    keyRow.append(keyInput, addKeyBtn, keyMsg);
    card.append(keyRow);

    // Lifecycle actions (bottom-right, icon buttons).
    const actRow = el('div', 'card-actions');
    const toggle = iconBtn('icon-power', t.status === 'active' ? '禁用' : '启用');
    toggle.classList.toggle('danger', t.status === 'active');
    toggle.addEventListener('click', async () => {
      try {
        await mg(`admin/tenants/${t.id}`, {
          method: 'PUT',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ status: t.status === 'active' ? 'disabled' : 'active' }),
        });
        toast(`已${t.status === 'active' ? '禁用' : '启用'} ${t.id}`);
        await renderTenant();
      } catch (e) { toast('状态切换失败: ' + e.message, 'bad'); }
    });
    const del = iconBtn('icon-trash', '删除');
    del.classList.add('danger');
    del.addEventListener('click', async () => {
      if (!(await confirmDeleteTenant(t))) return;
      try {
        await mg(`admin/tenants/${t.id}`, { method: 'DELETE' });
        toast(`已删除 ${t.id}`);
        await renderTenant();
      } catch (e) { toast('删除失败: ' + e.message, 'bad'); }
    });
    actRow.append(toggle, del);
    card.append(actRow);
    list.append(card);
  }
  sec.append(list);
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
  applyTheme(localStorage.getItem('consoleTheme') || 'midnight');
  $('#btn-settings').addEventListener('click', openSettings);
  $('#btn-logout').addEventListener('click', logout);
  $('#settings-close').addEventListener('click', closeSettings);
  $('#settings-overlay').addEventListener('click', e => { if (e.target === e.currentTarget) closeSettings(); });
  await initClusters();
  buildSparqlTab();
  setInterval(probeStatus, refreshMs > 0 ? refreshMs : 5000);
  probeStatus();
  render();
})();
