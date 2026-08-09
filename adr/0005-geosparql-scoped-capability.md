# ADR-0005: GeoSPARQL 范围能力（Profile-gated）

- Status: Accepted
- Date: 2026-08-09
- Deciders: sharky-ai（项目负责人；Codex 执行体代为执行决策流程）
- Tags: r3, query, geosparql, profile

## Context

PLAN-0001 §6 R3 要求交付「GeoSPARQL 范围能力」。SAS-0001 §3 允许
GeoSPARQL 以 profile-gated 模块交付，且任何未支持要素必须产生确定性、
文档化的错误。约束：

- Rust-only 生产路径，零新增外部依赖优先（L8 先例：语义索引零外部依赖）。
- 跨进程、跨版本字节级确定性（R1–R2 既有门禁文化）。
- W3C 492/492 + SHACL 98/98 零漂移必须保持。
- 范围必须可验收：明确支持集、明确非目标。

## Decision

1. 交付 **Point + Rectangle（轴对齐矩形）** 范围几何剖面，闭合的 DE-9IM
   sf 拓扑代数；WKT（`geo:wktLiteral`，CRS84）与 GeoJSON
   （`geo:jsonLiteral`）两种序列化。
2. 新 crate `crates/ontolith-geo`（零外部依赖）承载几何模型、解析、
   序列化、haversine 距离与拓扑；`ontolith-query` path 依赖接入。
3. 表达式函数按 GeoSPARQL 1.1 函数 IRI 接线：`geof:distance`、
   `geof:envelope`、`geof:getSRID`、`geof:isSimple`、`geof:isValid`、
   `geof:sf{Equals,Disjoint,Intersects,Touches,Crosses,Within,Contains,Overlaps}`。
4. `geo:asWKT` / `geo:asGeoJSON` / `geo:hasGeometry` 作为基本图模式
   属性函数（subject 已绑定方向）经 `eval_bgp` 重写；存储中显式同谓词
   三元组照常匹配，互不干扰。
5. 非范围要素（任意多边形、LINESTRING、MULTI-*、非 CRS84、未知单位、
   buffer 等）→ 确定性错误（SPARQL 表达式错误语义：unbound，不使查询失败）。
6. 验收门禁：`ontolith-compliance` `r3_geo_gate`（确定性/拓扑表/距离/
   属性函数）+ CI `r3-gates` 作业；workspace 全量 + W3C 492/492 +
   SHACL 98/98 零漂移 + clippy 零警告 + fmt 对齐。

## Consequences

### Positive

- 可验收的确定性范围剖面，符合 SAS-0001 profile-gated 条款。
- 零外部依赖、跨进程确定性，与既有架构约束一致。
- 覆盖常见空间过滤/连接场景（点-点距离、包围盒相交/包含）。

### Negative / risks

- 非完整 GeoSPARQL 一致性；复杂几何用户需扩展（未来 Polygon/LINESTRING
  可作为剖面演进，仍保持确定性错误底线）。
- 线性扫描无空间索引；大数据量检索性能受限（范围剖面接受）。

### Mitigations

- 非目标显式写入 [L9-geosparql.md](../docs/L9-geosparql.md) §2。
- 确定性错误文案固化并由门禁断言；未来扩展不破坏既有语义。

## Alternatives considered

| Option | Why not now |
|--------|-------------|
| 引入 `geo`/`geos` 外部计算几何库 | 违反零新增外部依赖偏好；需 Tier A/B 流程与回退方案；范围剖面用不到全量能力 |
| 直接实现任意 Polygon 拓扑 | DE-9IM 全量复杂度高、验收面大；与「范围能力」定位不符，留作剖面演进 |
| 仅 Point 剖面 | 缺少包围盒过滤（GeoSPARQL 最常见用例），范围价值不足 |

## References

- [L9-geosparql.md](../docs/L9-geosparql.md)
- [SAS-0001 §3](./../docs/Ontolith_Software_Architecture_Specification.md)
- [PLAN-0001 §6 R3](./../docs/Ontolith_Development_Plan.zh-CN.md)
- OGC GeoSPARQL 1.1（WKT/GeoJSON 序列化、sf 拓扑、geof 函数 IRI）
