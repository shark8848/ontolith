# L3 — Parser & Query Engine 完整功能说明

文档 ID: IMPL-L3-0001  
版本: 2.1.0  
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
| timeout + 协作式 cancel | ✅ |
| 经 L2 SPO/POS/OSP 访问 | ✅ |
| JSON-LD | ❌ 明确 Unsupported |
| SPARQL Update / DESCRIBE 执行 | ❌ 解析识别，执行 Unsupported |
| 属性路径 / 子查询 / EXISTS / 完整聚合（GROUP BY/HAVING） / 服务联邦 | ❌ 后续增强 |
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
| SELECT (COUNT(...) AS ?x)（无 GROUP BY） | ✅ |
| ASK WHERE { ... } | ✅ → `boolean` |
| CONSTRUCT { template } WHERE { ... } | ✅ → `construct_triples` |
| DESCRIBE / UPDATE | 识别 kind，执行 `Unsupported` |
| PREFIX / BASE | ✅ |

### 3.3 图模式

| 构造 | 代数 | 执行 |
|------|------|------|
| 三元组模式序列 | `Bgp` | 逐模式求精；SPO/POS/OSP 选路 |
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
- 结果标志 `timed_out` / `cancelled`  

### 3.10 Explain

```rust
pipeline.explain(&req)? -> QueryExplain {
  plan_id, kind, logical_steps, physical_steps, algebra_summary
}
```

logical 含 `optimize:before->after`。

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
| query | 24 | SELECT/JOIN/OPTIONAL/UNION/FILTER/BIND/VALUES/CONSTRUCT/ASK/DISTINCT/ORDER/LIMIT/PREFIX/COUNT(无 GROUP BY)/Explain/timeout/cancel/txn/hint |
| storage 回归 | 24 | 绿 |
| core 回归 | 11 | 绿 |

---

## 6. 已知限制（完整 L3 边界，非“未开工”）

1. **属性路径**、**子查询**、**EXISTS/NOT EXISTS**、**GROUP BY/HAVING 与其他聚合函数**、**SERVICE** 未实现（仅 COUNT 无 GROUP BY 基线已支持）。  
2. **SPARQL Update / DESCRIBE** 仅 kind 识别。  
3. **JSON-LD** 未实现。  
4. JOIN 为嵌套循环式 solution merge（正确优先，非代价模型）。  
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
