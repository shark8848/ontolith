#!/usr/bin/env node
// SPARQL loadtest sample runner (L3/console): runs the 10 documented SPARQL
// samples against a gateway from console/clusters.json and rewrites the
// verification table in docs/sparql-samples-loadtest.md.
// Usage: node scripts/sparql-samples-loadtest.mjs [clusters.json] [output.md]
import fs from 'node:fs';

const clustersPath = process.argv[2] ?? 'console/clusters.json';
const outPath = process.argv[3] ?? 'docs/sparql-samples-loadtest.md';
const clusters = JSON.parse(fs.readFileSync(clustersPath, 'utf8'));
const c = clusters.find((x) => x.id === 'prod');
if (!c) {
  console.error(`no "prod" cluster in ${clustersPath}`);
  process.exit(1);
}
const BASE = c.gateway.replace(/\/$/, '');
const HDR = { 'x-api-key': c.apiKey, 'x-ontolith-tenant': c.tenant, 'x-ontolith-user': c.user };

const P = 'PREFIX ex: <http://ontolith.example/loadtest/> ';
const samples = [
  ['1 基础计数', P + 'SELECT (COUNT(?s) AS ?total) WHERE { ?s ?p ?o }'],
  ['2 谓词分布', P + 'SELECT ?p (COUNT(?s) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY DESC(?n)'],
  ['3 标签模糊检索', P + 'SELECT ?s ?o WHERE { ?s ex:label ?o . FILTER(CONTAINS(STR(?o), "LoadTest-00025")) } LIMIT 10'],
  ['4 整数区间过滤', P + 'SELECT ?s ?v WHERE { ?s ex:value ?v . FILTER(?v >= 4000 && ?v < 4500) } ORDER BY ?v LIMIT 20'],
  ['5 小数过滤排序', P + 'SELECT ?s ?price WHERE { ?s ex:price ?price . FILTER(?price >= 60) } ORDER BY DESC(?price) LIMIT 10'],
  ['6 owner 分组统计', P + 'SELECT ?owner (COUNT(?s) AS ?n) WHERE { ?s ex:owner ?owner } GROUP BY ?owner ORDER BY DESC(?n) LIMIT 10'],
  ['7 状态分组', P + 'SELECT ?st (COUNT(?s) AS ?n) WHERE { ?s ex:status ?st } GROUP BY ?st ORDER BY ?st'],
  ['8 主语前缀过滤', P + 'SELECT ?s ?st WHERE { ?s ex:status ?st . FILTER(STRSTARTS(STR(?s), "http://ontolith.example/loadtest/row-009")) } ORDER BY ?s LIMIT 10'],
  ['9 最大值排序', P + 'SELECT ?s ?v WHERE { ?s ex:value ?v } ORDER BY DESC(?v) LIMIT 5'],
  ['10 UNION 多模式', P + 'SELECT ?s ?kind WHERE { { ?s ex:label ?o BIND("label" AS ?kind) } UNION { ?s ex:value ?v BIND("value" AS ?kind) } } ORDER BY ?s LIMIT 10'],
];

async function run(q) {
  const t0 = Date.now();
  try {
    const res = await fetch(`${BASE}/sparql?query=${encodeURIComponent(q)}`, { headers: HDR });
    const j = await res.json();
    const ms = Date.now() - t0;
    if (!res.ok) return { ok: false, ms, err: j.error || res.status };
    const rows = j.results?.bindings?.length ?? 0;
    const first = rows ? j.results.bindings[0] : null;
    const sum = first
      ? Object.entries(first).map(([, v]) => `${v.type === 'uri' ? v.value : v.value}`).join(' ')
      : '';
    return { ok: true, ms, rows: j.results?.bindings?.length, count: j.meta?.row_count, sum };
  } catch (e) {
    return { ok: false, ms: Date.now() - t0, err: e.message };
  }
}

const lines = [];
for (const [name, q] of samples) {
  const r = await run(q);
  lines.push(
    `${name} | ${r.ok ? 'OK' : 'FAIL'} | ${r.ms}ms | rows=${r.rows ?? '-'}${r.count !== undefined ? ' (count ' + r.count + ')' : ''}${r.sum ? ' | ' + r.sum : ''}${r.err ? ' | err=' + r.err : ''}`
  );
  console.log(lines.at(-1));
}
const md =
  '# SPARQL 测试样例（loadtest 10000 条）\n\n' +
  lines
    .map((l, i) => `## ${samples[i][0]}\n\`\`\`sparql\n${samples[i][1]}\n\`\`\`\n\n验证：${l.split(' | ').slice(1).join(' | ')}\n`)
    .join('\n');
fs.writeFileSync(outPath, md);
console.log(`wrote ${outPath}`);
