# L3 — Parser & Query Engine 完整功能说明

文档 ID: IMPL-L3-0001  
版本: 2.9.0  
状态: Implemented (full L3 core, not MVP-only)  
日期: 2026-07-22  
对应 crate:

- `crates/ontolith-parser`
- `crates/ontolith-query`

规范依据:

- [SAS-0001](./Ontolith_Software_Architecture_Specification.md) §7 Query Pipeline
- [L0](./L0-ontolith-core-Knowledge-Object-Foundation.md) · [L1](./L1-ontolith-rdf-Statement-Graph-Dataset.md) · [L2](./L2-ontolith-storage-transaction-kernel.md)
- [PLAN-0001 Phase 3 / WBS-02 / WBS-04](./Ontolith_Development_Plan.zh-CN.md)

---

## 1. 层定位与完成定义

```text
SPARQL / RDF text
        │
   Lexer → Parser → AST/Algebra
        │
   Rule Optimizer
        │
   Physical Plan (index access)
        │
   Executor → Solutions / ASK / CONSTRUCT
        │
   L2 Storage (SPO/POS/OSP)
```

**本层完成定义（相对架构手册，非“最小能跑”）：**

| 能力域 | 状态 |
|--------|------|
| RDF 交换语法 N-Triples / N-Quads / Turtle / TriG | ✅ |
| 流式解析事件 `RdfEvent` + Sink | ✅ |
| 结构化解析错误（行/列） | ✅ |
| SPARQL SELECT / ASK / CONSTRUCT 核心 | ✅ |
| WHERE 组、OPTIONAL、UNION、FILTER、BIND、VALUES | ✅ |
| DISTINCT / ORDER BY / LIMIT / OFFSET | ✅ |
| PREFIX / BASE | ✅ |
| 代数 + 规则优化 + Explain | ✅ |
| 解绑定 `Solution` 结果（非仅 row_count） | ✅ |
| timeout + 协作式 cancel + 异步抢占 token | ✅ |
| 经 L2 SPO/POS/OSP 访问 | ✅ |
| 属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`） | ✅ |
| RDF 序列化导出（N-Triples / N-Quads 写出，`SerializeFormat`） | ✅ |
| JSON-LD | ❌ 明确 Unsupported |
| SPARQL Update（INSERT DATA / DELETE DATA / DELETE·INSERT…WHERE / DELETE WHERE） | ✅ |
| SPARQL Update 高级形态（CLEAR/DROP 图作用域、WITH 图作用域 DELETE·INSERT…WHERE、LOAD 本地图复制） / DESCRIBE 执行 | ✅（LOAD 为本地命名图复制子集，远程 HTTP 抓取未实现） |
| 属性路径扩展（分组/嵌套更完整 1.1 语法） | ❌ 后续增强 |
| 完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合） | ✅ |
| SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE） | ✅ |
| 高级子查询（相关子查询等） / EXISTS / 服务联邦 | ❌ 后续增强 |
| 流式 Result 协议（网络层） | ❌ 属 L5 接入层 |

---

## 2. Parser（`ontolith-parser`）

### 2.1 模块

```text
domain/           ParseFormat/Request/Stats/Output, RdfEvent, DatasetSink
application/      RdfParser trait (parse + parse_streaming)
infrastructure/
  term_lex.rs     共享 Lexer / PrefixMap / 字面量与前缀展开
  nt.rs           N-Triples / N-Quads（流式）
  turtle.rs       Turtle + TriG
  mod.rs          BasicRdfParser 统一入口
```

### 2.2 已实现语法

#### N-Triples / N-Quads

- IRI、blank、简单/语言/类型字面量  
- N-Quads 图名（默认图或缺省第四位）  
- 行注释 `#`  
- 流式 `parse_document_streaming`

#### Turtle

- `@prefix` / `PREFIX`、`@base` / `BASE`  
- 前缀名、`a`、绝对 IRI  
- 谓词列表 `;`、对象列表 `,`  
- 短/长字符串、语言标签、`^^` 类型  
- 空白节点 `_:x`、`[]` 属性表  
- 集合 `( a b c )` → `rdf:first` / `rdf:rest` / `rdf:nil`  
- 数值与布尔字面量  

#### TriG

- 命名图 `iri { ... }` / `GRAPH iri { ... }`  
- 默认图 `{ ... }`  
- 与 Turtle 指令共存  

#### JSON-LD

- 返回 `OntolithError::Unsupported("json-ld")`

### 2.3 流式契约

```rust
pub enum RdfEvent { Triple, Quad, Prefix, Base, Comment }

pub trait RdfEventSink {
    fn on_event(&mut self, event: RdfEvent) -> Result<(), OntolithError>;
}
```

`DatasetSink` 将事件归集为 `Dataset` + `ParseStats`。

### 2.4 错误

使用 `OntolithError::Failed` / `parse_at(line, col, msg)`，含位置信息。

### 2.5 字典

所有主语/空白节点经 `DictionaryCodec::encode_node` 得到稳定 `NodeId`；blank lexical 为 `_:label`。

---

## 3. Query（`ontolith-query`）

### 3.1 流水线（对齐 SAS-0001 §7）

```text
Query text
  → SparqlParser (lexer+parser)
  → Algebra
  → RuleBasedOptimizer
  → AlgebraExecutor (physical index access)
  → QueryResult { solutions | boolean | construct_triples }
```

入口：`infrastructure::standard_pipeline(repo)`  
或 `QueryPipeline::new(SimpleQueryPlanner, RuleBasedOptimizer, ReadServiceQueryExecutor)`。

### 3.2 SPARQL 查询形态

| 形态 | 支持 |
|------|------|
| SELECT [DISTINCT] * / ?vars | ✅ |
| SELECT (AGG(...) AS ?x)（COUNT/SUM/AVG/MIN/MAX、COUNT(DISTINCT)） | ✅ |
| GROUP BY ?v / (expr AS ?alias) + HAVING（可引用聚合别名或 `SUM(?v) > n`） | ✅ |
| 嵌套子查询 `{ SELECT ... LIMIT ... }`（基线） | ✅ |
| 子查询内聚合 + 外层继续聚合 | ✅ |
| INSERT DATA / DELETE DATA（具体三元组，变量与 blank 拒绝） | ✅ |
| DELETE { tpl } INSERT { tpl } WHERE { pattern } / DELETE WHERE { pattern } | ✅ |
| ASK WHERE { ... } | ✅ → `boolean` |
| CONSTRUCT { template } WHERE { ... } | ✅ → `construct_triples` |
| DESCRIBE | 识别 kind，执行 `Unsupported` |
| UPDATE（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE） | ✅ → `affected` 计数 |
| PREFIX / BASE | ✅ |

### 3.3 图模式

| 构造 | 代数 | 执行 |
|------|------|------|
| 三元组模式序列 | `Bgp` | 逐模式求精；SPO/POS/OSP 选路 |
| 属性路径最小集 `p1/p2`、`+`、`*`、`|`、`^` | `Path` + `PathExpression` | 递归求值 + 字典桥接（IRI↔Node） |
| 并列模式 | `Join` | 哈希兼容 join（solution merge） |
| OPTIONAL | `LeftJoin` | 左外连接 |
| UNION | `Union` | 多重集合并 |
| FILTER | `Filter` | 表达式布尔过滤 |
| BIND (expr AS ?v) | `Extend` | 扩展绑定 |
| VALUES | `Values` | 内联绑定表 |
| 嵌套 `{ }` | 递归 group | ✅ |

### 3.4 表达式（FILTER/BIND）

- `BOUND`、`isIRI`/`isURI`、`isLiteral`、`isBlank`  
- `!` / `NOT`、`&&`/`AND`、`||`/`OR`  
- `=` `!=` `<` `<=` `>` `>=`  
- 变量、IRI、字面量  

### 3.5 解修饰符

- `DISTINCT`  
- `ORDER BY [ASC|DESC] ?v`  
- `LIMIT` / `OFFSET`（可任意顺序出现）  
- `Project`（SELECT 变量列表或 `*`）  

### 3.6 结果模型

```rust
pub struct Solution { bindings: BTreeMap<String, BoundValue> }
pub enum BoundValue { Node, Iri, Literal, Blank }

pub struct QueryResult {
    kind, variables, solutions,
    boolean,              // ASK
    construct_triples,    // CONSTRUCT
    elapsed_ms, timed_out, cancelled,
}
```

兼容：`QueryResultSummary` + `execute_summary()`。

### 3.7 优化器（规则）

`RuleBasedOptimizer`：

1. 消除 `Identity` 单元  
2. 合并相邻 `Join(Bgp,Bgp)` 并按绑定位置重排 BGP（S→P→O）  
3. Filter 穿越 Distinct 的下推  
4. 刷新 physical_steps  

### 3.8 物理访问

| 绑定 | 索引 |
|------|------|
| subject `NodeId` | SPO |
| predicate IRI | POS |
| object term | OSP |
| 无绑定 | 全表扫描 |

### 3.9 Timeout / Cancel

- `QueryRequest.timeout_ms`：`0` 立即超时；执行中协作检查  
- `QueryRequest.cancel: Arc<AtomicBool>`：协作取消  
- `PreemptionToken`：墙钟 deadline + 共享 cancel 标志，`reason()` 区分 Timeout/Cancelled；可从其它线程 `preempt()` 异步抢占  
- 抢占轮询粒度：BGP 候选三元组、join 内层行、FILTER/EXTEND/VALUES 行、Update 各 op（不再仅按 pattern 粒度）  
- 结果标志 `timed_out` / `cancelled`  

### 3.10 Explain

```rust
pipeline.explain(&req)? -> QueryExplain {
  plan_id, kind, logical_steps, physical_steps, algebra_summary,
  estimated_rows,          // 代价优化器估算结果行数（无统计时为 None）
  pattern_costs            // 每三元组模式：pattern/selectivity/estimated_rows
}
```

logical 含 `optimize:before->after`（代价优化为 `optimize(cost):...`）。HTTP `/explain` 输出 JSON 含 `estimated_rows` 与 `pattern_costs`。

### 3.11 其它

- 遗留 `# subject=N` 提示：特化 WHERE 中首个未绑定 subject  
- 测试辅助词法 `node:123` → `TermPattern::Node`  

---

## 4. 错误模型扩展（L0 联动）

`OntolithError` 新增：

- `Failed(String)` 动态诊断  
- `parse_at(line, col, msg)`  
- `query(msg)`  

静态变体保持兼容。

---

## 5. 测试验收

| Crate | 测试数 | 覆盖 |
|-------|--------|------|
| parser | 11 | NT/NQ/Turtle/TriG/集合/blank 属性表/流式/定位错误/JSON-LD |
| query | 46 | SELECT/JOIN/OPTIONAL/UNION/FILTER/BIND/VALUES/CONSTRUCT/ASK/DISTINCT/ORDER/LIMIT/PREFIX/完整聚合（GROUP BY/HAVING、COUNT(DISTINCT)/SUM/AVG/MIN/MAX、子查询聚合）/SPARQL Update（INSERT/DELETE DATA、DELETE·INSERT…WHERE、DELETE WHERE）/子查询基线/属性路径最小集（`/`、`+`、`*`、`?`、`|`、`^`）/Explain/timeout/cancel/txn/hint |
| storage 回归 | 24 | 绿 |
| core 回归 | 11 | 绿 |

---

## 6. 已知限制（完整 L3 边界，非“未开工”）

1. **属性路径扩展（分组/嵌套更完整 1.1 语法）**、**高级子查询（相关子查询等）**、**EXISTS/NOT EXISTS**、**SERVICE** 未实现（已支持完整聚合 GROUP BY/HAVING、嵌套 SELECT+LIMIT 子查询、子查询聚合与属性路径最小集 `p1/p2`、`+`、`*`、`?`、`|`、`^`）。HAVING 中聚合调用需匹配投影聚合表达式（重写为别名求值）。  
2. **SPARQL Update 高级形态**：已支持 `CLEAR/DROP [SILENT] DEFAULT|NAMED|ALL|GRAPH <g>`、`WITH <g>` 作用于 DELETE·INSERT…WHERE / DELETE WHERE（WHERE 以图 `g` 为默认图匹配，模板写入图 `g`）、`LOAD [SILENT] <src> [INTO GRAPH <g>]`（离线子集：`<src>` 为库内已有命名图，复制到默认图或目标图；远程 HTTP 抓取留待网络层）。`WITH` 仅组合 modify 形态（与规范一致）；DELETE/INSERT 模板中的 blank 节点按未绑定处理（跳过该三元组）；无匹配的更新为空操作不报错。  
3. **JSON-LD** 未实现。  
4. JOIN 为嵌套循环式 solution merge；BGP 模式序由代价优化器按实时统计（triple/predicate/subject/object 计数）做贪心选序 + 绑定传播（`EngineQueryStatistics` + `CostBasedOptimizer`），统计为均匀选择性启发式，尚无采样/直方图。  
5. CONSTRUCT 模板中的 blank 生成语义为绑定投影，非全规范 blank 唯一化。  
6. 网络流式结果属于 **L5 server**，本层交付内存 `QueryResult`。  

---

## 7. 代码索引

| 主题 | 路径 |
|------|------|
| Turtle/TriG | `crates/ontolith-parser/src/infrastructure/turtle.rs` |
| 共享词法 | `crates/ontolith-parser/src/infrastructure/term_lex.rs` |
| N-T/N-Q 流式 | `crates/ontolith-parser/src/infrastructure/nt.rs` |
| SPARQL 解析 | `crates/ontolith-query/src/infrastructure/sparql_parse.rs` |
| 规则优化 | `crates/ontolith-query/src/infrastructure/optimize.rs` |
| 执行器 | `crates/ontolith-query/src/infrastructure/execute.rs` |
| 标准流水线 | `standard_pipeline` in query infrastructure |

---

## 8. 变更记录

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-07-17 | 1.0.0 | MVP 子集 |
| 2026-07-17 | 2.0.0 | 完整 L3：Turtle/TriG/流式；SPARQL 代数全核心；优化；解绑定；cancel |
| 2026-07-22 | 2.1.0 | 新增 COUNT 聚合最小能力（无 GROUP BY）与对应测试；文档同步已知限制 |
| 2026-07-22 | 2.2.0 | 新增嵌套 SELECT+LIMIT 子查询基线与对应测试；W3C subset 子查询用例可通过 |
| 2026-07-22 | 2.3.0 | 新增属性路径序列（iri/iri）基线与对应测试；W3C subset 属性路径基线用例可通过 |
| 2026-07-22 | 2.4.0 | 新增属性路径高级算子最小集（`+`、`*`、`|`、`^`）与对应测试；W3C subset 路径最小集用例可通过 |
| 2026-08-06 | 2.5.0 | 新增 RDF 序列化导出（`domain/serialize.rs`：N-Triples/N-Quads 写出、字面量转义与确定性词法、Dataset 按格式过滤），+5 测 |
| 2026-08-06 | 2.6.0 | 新增属性路径 `?`（zero-or-one）：解析（修饰符紧贴 IRI，避免与变量 `?x` 混淆）、执行（自身 ∪ 单步去重）、W3C 子集新增 must-pass 用例（25/25），+2 测 |
| 2026-08-06 | 2.7.0 | 新增完整聚合：投影聚合表达式（COUNT/SUM/AVG/MIN/MAX、COUNT(DISTINCT)）解析、GROUP BY（变量或 `(expr AS ?alias)`）、HAVING（聚合调用重写为投影别名）、子查询聚合；执行器按组求值（SUM 整数保精、AVG 十进制、MIN/MAX 序比较）；query 32→39 测，W3C 子集 must-pass 25→27/27，全量测试 190 通过 |
| 2026-08-06 | 2.8.0 | 新增 SPARQL Update：解析 INSERT DATA / DELETE DATA / DELETE·INSERT…WHERE / DELETE WHERE（LOAD/CLEAR/WITH 明确 Unsupported）；`UpdateOp` 域模型 + `QueryResult.affected`；`UpdateWriteService`/`UpdateQueryExecutor`（字典 IRI→NodeId 桥、单事务写、失败回滚）；server 接入写管线；query 39→46 测，W3C 子集 must-pass 27→30/30（skip=0），全量测试 199 通过 |
| 2026-08-06 | 2.9.0 | 完整 W3C SPARQL 1.1 套件接入（vendored `w3c/rdf-tests` sparql11，941 文件/28 feature）：manifest 驱动 runner `ontolith-compliance/tests/w3c11_suite.rs` 执行 QueryEvaluation/UpdateEvaluation/PositiveSyntax/NegativeSyntax 四类，SRX/SRJ/TSV/CSV + Turtle 图 + ASK 结果比对；`w3c11_profile.tsv` 锁定 492 条基线（127 PASS/365 FAIL，reason-code 分类：parse-error 223 / data-format 52 / semantic 48 / accepted-invalid 17 / named-graph 16 / other 9），drift 防回归；修复 Turtle 数字字面量词法（`.` 不再作分隔符，完整 INTEGER/DECIMAL/DOUBLE 文法 + `.5`），parser 16→17 测 |
| 2026-08-07 | 2.10.0 | 新增 SPARQL Update 高级形态：`CLEAR/DROP [SILENT] DEFAULT/NAMED/ALL/GRAPH <g>`、`WITH <g>` 图作用域 DELETE·INSERT…WHERE / DELETE WHERE（WHERE 以图 g 为默认图、模板写入图 g）、`LOAD [SILENT] <src> [INTO GRAPH <g>]` 本地命名图复制子集；修复空更新（无匹配 DELETE/CLEAR 空图）误报 “pending storage transaction not found” 的 bug（无写入不提交）；`UpdateWriteService` 增图范围读取（default/named/graph）；query 46→55 测，W3C 套件基线 127→151 PASS（24 项 FAIL→PASS：clear/drop 5、delete/delete-insert/delete-where 4、syntax-update 13、update-silent 2），无回归 |
| 2026-08-07 | 2.11.0 | 新增代价模型/统计：`QueryStatistics` 契约（triple/subject/predicate/object 计数 + 均匀选择性 `pattern_selectivity`）、`EngineQueryStatistics`（引擎增量统计）、`CostBasedOptimizer`（贪心 join 序 + 绑定传播，语义保持）；`update_pipeline` 与新增 `cost_pipeline` 走代价优化；query 55→58 测，W3C 套件基线无漂移 |
| 2026-08-07 | 2.12.0 | Explain 增成本信息：`QueryExplain`/`QueryPlan` 新增 `estimated_rows`（主导 BGP 乘积×总数）与 `pattern_costs`（逐模式 selectivity/estimated_rows，代价优化器填充）；HTTP `/explain` JSON 输出两字段；query 58→59 测，server explain HTTP 测试 +1（20 测） |
| 2026-08-07 | 2.13.0 | 异步抢占 token：`PreemptionToken`（deadline + cancel 标志，`reason()` 区分 Timeout/Cancelled，`preempt()` 可跨线程触发）；执行器轮询粒度细化到 BGP 候选/join 行/FILTER/EXTEND/VALUES 行，Update 抢占返回 `timed_out`/`cancelled` 标志且不落写；query 59→63 测 |
