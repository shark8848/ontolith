'use strict';

const $ = (sel, root = document) => root.querySelector(sel);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
};

// ---------- fetch helpers ----------
async function api(path, opts = {}) {
  const res = await fetch(path, opts);
  const text = await res.text();
  let body;
  try { body = text ? JSON.parse(text) : null; } catch { body = text; }
  if (!res.ok) {
    const msg = (body && (body.error || body.message)) || `HTTP ${res.status}`;
    throw new Error(msg);
  }
  return body;
}
const gw = (p, o) => api('/api/gw/' + p, o);
const mg = (p, o) => api('/api/mg/' + p, o);

// ---------- state ----------
let refreshMs = 5000;
let activeTab = 'overview';
let autoTimer = null;

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

function startAuto(ms) {
  stopAuto();
  if (ms > 0) autoTimer = setInterval(render, ms);
}
function stopAuto() { if (autoTimer) { clearInterval(autoTimer); autoTimer = null; } }

// ---------- rendering ----------
async function render() {
  try {
    switch (activeTab) {
      case 'overview': await renderOverview(); break;
      case 'cluster': await renderCluster(); break;
      case 'data': await renderData(); break;
      case 'audit': await renderAudit(); break;
      case 'traces': await renderTraces(); break;
      case 'config': await renderConfig(); break;
    }
  } catch (err) {
    const sec = $('#tab-' + activeTab);
    sec.replaceChildren(el('p', 'err', '加载失败: ' + err.message));
  }
  if (activeTab === 'overview' || activeTab === 'cluster' || activeTab === 'data') {
    startAuto(refreshMs);
  }
}

async function probeStatus() {
  let health;
  try { health = await api('/api/health'); } catch { return; }
  if (health.refresh_ms) refreshMs = health.refresh_ms;
  const gwOk = health.gateway && health.gateway.status === 'ok';
  const mgOk = health.management && health.management.status === 'ok';
  setDot('dot-gw', gwOk ? 'ok' : 'bad');
  setDot('dot-mg', mgOk ? 'ok' : 'bad');
  const note = $('#refresh-note');
  if (gwOk && mgOk) note.textContent = '网关/管理面可达 · 自动刷新 ' + refreshMs + 'ms';
  else note.textContent = '部分组件不可达';
}
function setDot(id, state) {
  const d = $('#' + id);
  d.className = 'dot ' + state;
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

// ---------- overview ----------
async function renderOverview() {
  const sec = $('#tab-overview');
  sec.replaceChildren();
  const [gwHealth, mgHealth, mon, metrics] = await Promise.all([
    gw('health').catch(e => ({ error: e.message })),
    mg('admin/health').catch(e => ({ error: e.message })),
    mg('admin/monitoring').catch(e => ({ error: e.message })),
    api('/api/gw/metrics').catch(() => ''),
  ]);
  const cards = el('div', 'cards');
  cards.append(kvCard('Gateway', [
    ['状态', gwHealth.error ? '不可达' : (gwHealth.status || '—')],
    ['后端', gwHealth.backend || '—'],
    ['三元组', gwHealth.triples ?? '—'],
    ['四元组', gwHealth.quads ?? '—'],
    ['鉴权', gwHealth.auth_mode || '—'],
    ['tracing', gwHealth.tracing || '—'],
    ['数据目录', gwHealth.data_dir || '—'],
  ]));
  cards.append(kvCard('管理面', [
    ['状态', mgHealth.error ? '不可达' : (mgHealth.status || '—')],
    ['运行时长', mgHealth.uptime_ms ? (mgHealth.uptime_ms / 1000).toFixed(1) + 's' : '—'],
    ['runtime probe', mgHealth.runtime_probe?.reachable ? '可达 ' + (mgHealth.runtime_probe.latency_ms ?? 0) + 'ms' : '不可达'],
    ['jwt / oidc', `${mgHealth.jwt || 'off'} / ${mgHealth.oidc || 'off'}`],
  ]));
  cards.append(kvCard('监控摘要', [
    ['请求总数', mon.requests_total ?? '—'],
    ['SPARQL 请求', mon.sparql_total ?? '—'],
    ['SPARQL 错误', mon.sparql_errors ?? '—'],
    ['ingest 总数', mon.ingest_total ?? '—'],
    ['平均延迟', mon.latency_avg_ms !== undefined ? mon.latency_avg_ms + 'ms' : '—'],
    ['集群节点', mon.cluster ? `${mon.cluster.healthy}/${mon.cluster.nodes}` : '—'],
  ]));
  sec.append(cards);

  const m = el('div', 'card');
  m.append(el('h3', null, 'Prometheus 指标（网关）'));
  const lines = String(metrics || '').split('\n').filter(l => l && !l.startsWith('#'));
  const tbl = el('table');
  const thead = el('thead');
  const hr = el('tr');
  ['指标', '值', '时间戳'].forEach(h => hr.append(el('th', null, h)));
  thead.append(hr); tbl.append(thead);
  const tbody = el('tbody');
  for (const line of lines.slice(0, 40)) {
    const parts = line.split(/\s+/);
    if (parts.length < 2) continue;
    const tr = el('tr');
    tr.append(el('td', null, parts[0]));
    tr.append(el('td', null, parts[1]));
    tr.append(el('td', null, parts[2] ? fmtTs(parts[2]) : '—'));
    tbody.append(tr);
  }
  tbl.append(tbody);
  m.append(tbl);
  sec.append(m);
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
  cards.append(kvCard('集群状态', st.error
    ? [['错误', st.error]]
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

// ---------- SPARQL ----------
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
  const msg = el('span', 'muted');
  row.append(runBtn, expBtn, msg);
  sec.append(row);
  const out = el('div');
  sec.append(out);

  async function execute(explain) {
    const q = box.value.trim();
    if (!q) { msg.textContent = '请输入查询'; return; }
    runBtn.disabled = expBtn.disabled = true;
    msg.textContent = '执行中…';
    try {
      const res = await api('/api/gw/' + (explain ? 'explain' : 'sparql') + '?query=' + encodeURIComponent(q));
      out.replaceChildren();
      if (explain) {
        out.append(el('pre', 'json', typeof res === 'string' ? res : JSON.stringify(res, null, 2)));
      } else {
        renderSparqlResult(out, res);
      }
      msg.textContent = '';
    } catch (err) {
      msg.textContent = '错误: ' + err.message;
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
  const thead = el('thead');
  const hr = el('tr');
  head.forEach(v => hr.append(el('th', null, v)));
  thead.append(hr); tbl.append(thead);
  const tbody = el('tbody');
  for (const b of rows) {
    const tr = el('tr');
    for (const v of head) {
      const cell = b[v];
      const td = el('td');
      if (!cell) { td.textContent = ''; }
      else if (cell.type === 'uri') { td.append(el('code', null, cell.value)); }
      else if (cell.type === 'literal') {
        td.append(el('span', null, cell.value));
        if (cell.datatype && cell.datatype !== 'http://www.w3.org/2001/XMLSchema#string') {
          td.append(el('span', 'tag', '^' + cell.datatype.split(/[#/]/).pop()));
        }
        if (cell['xml:lang']) td.append(el('span', 'tag', '@' + cell['xml:lang']));
      } else {
        td.append(el('span', 'tag', cell.type), el('span', null, ' ' + (cell.value || '')));
      }
      tr.append(td);
    }
    tbody.append(tr);
  }
  tbl.append(tbody);
  out.append(tbl);
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
      const res = await gw('data/turtle', {
        method: 'POST',
        headers: { 'content-type': 'text/turtle' },
        body,
      });
      out.textContent = JSON.stringify(res, null, 2);
      msg.textContent = '写入成功';
      renderData();
    } catch (err) {
      msg.textContent = '错误: ' + err.message;
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
    tr.append(el('td', null, fmtTs(ev.ts)));
    tr.append(el('td', null, ev.tenant || '—'));
    tr.append(el('td', null, ev.user || '—'));
    tr.append(el('td', null, ev.action || '—'));
    tr.append(el('td', null, ev.resource || '—'));
    tr.append(el('td', null, ev.outcome || '—'));
    tr.append(el('td', null, ev.detail || '—'));
    tbody.append(tr);
  }
  tbl.append(tbody);
  sec.append(tbl);
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
    tbl.append(tbody);
    lc.append(tbl);
    sec.append(lc);
  }
}

// ---------- boot ----------
if (!document.querySelector('#tab-sparql').hasChildNodes()) buildSparqlTab();
setInterval(probeStatus, refreshMs > 0 ? refreshMs : 5000);
probeStatus();
render();
