# L9 — GeoSPARQL 范围能力（R3）

- 文档 ID：L9-GEOSPARQL-0001
- 版本：1.0.0（2026-08-09 定稿）
- 状态：Approved
- 关联：PLAN-0001 §6 R3、SAS-0001 §3（profile-gated 模块）、ADR-0005
- 里程碑：R3-01 范围几何模型与序列化 → R3-02 `geof:`/`sf:` 函数 → R3-03 `geo:` 属性函数与查询接线 → R3-04 门禁与文档

## 1. 背景与目标

PLAN-0001 §6 R3 将「GeoSPARQL 范围能力」列为本阶段交付。SAS-0001 §3 允许
OWL 2 RL / SHACL / GeoSPARQL 以 profile-gated 模块交付，未支持的要素必须
产生确定性、文档化的错误。本设计定义 Ontolith 的 GeoSPARQL 范围剖面：

- 几何类型：**Point** 与 **Rectangle（轴对齐矩形）**，闭合的拓扑代数。
- 序列化：WKT（`geo:wktLiteral`，默认 CRS84）与 GeoJSON（`geo:jsonLiteral`）。
- 表达式函数：`geof:distance` / `geof:envelope` / `geof:getSRID` /
  `geof:isSimple` / `geof:isValid` / `geof:sfEquals` / `geof:sfDisjoint` /
  `geof:sfIntersects` / `geof:sfTouches` / `geof:sfCrosses` /
  `geof:sfWithin` / `geof:sfContains` / `geof:sfOverlaps`。
- 属性函数（BGP 重写）：`geo:asWKT` / `geo:asGeoJSON` / `geo:hasGeometry`
  （subject 已绑定的前向方向）。
- 零新增外部依赖：计算几何为树内实现（`ontolith-geo` crate），跨进程确定性。

## 2. 非目标（明确不做）

- 完整 OGC GeoSPARQL 1.1 一致性套件（Polygon 任意形状、LINESTRING、曲线、
  MULTI-*、GEOMETRYCOLLECTION、RCC8/Egenhofer 全拓扑、空间索引、buffer 等）。
- 非 CRS84/4326 投影计算（其他 CRS 一律确定性错误）。
- 栅格（Coverage）与拓扑（Topology）本体。
- 空间索引（R-tree 等）；范围剖面按数据量线性扫描即可满足 KPI 基线。

## 3. 几何模型与序列化

### 3.1 几何类型

```text
Geometry := Point { x: f64, y: f64 }
          | Rect  { xmin: f64, ymin: f64, xmax: f64, ymax: f64 }
```

- Point：单个坐标对。
- Rect：轴对齐矩形（`xmin ≤ xmax`、`ymin ≤ ymax` 强制）。
- 解析失败的输入返回确定性错误（错误类型见 §6）。

### 3.2 WKT（`http://www.opengis.net/ont/geosparql#wktLiteral`）

支持（可选 `<CRS>` 前缀，CRS 仅允许空（默认 CRS84）或
`<http://www.opengis.net/def/crs/OGC/1.3/CRS84>`）：

- `POINT (x y)` → Point
- `POLYGON ((xmin ymin, xmax ymin, xmax ymax, xmin ymax, xmin ymin))` →
  Rect（必须为 5 点、首尾闭合、轴对齐；否则确定性错误「非范围多边形」）
- `ENVELOPE (xmin ymin xmax ymax)`（GeoSPARQL 1.1 ENVELOPE 扩展）→ Rect

### 3.3 GeoJSON（`http://www.opengis.net/ont/geosparql#jsonLiteral`）

支持：

- `{"type":"Point","coordinates":[x,y]}` → Point
- `{"type":"Polygon","coordinates":[[[xmin,ymin],[xmax,ymin],[xmax,ymax],[xmin,ymax],[xmin,ymin]]]}` → Rect
- `"crs"` 成员若存在且非 CRS84 默认 → 确定性错误。

## 4. 函数语义

### 4.1 `geof:distance(?a, ?b, ?units)`

- 仅 Point–Point，CRS84（经纬度，WGS84）。
- 大圆（haversine）距离；`?units` 支持
  `http://www.opengis.net/def/uom/OGC/1.0/metre`（默认）与
  `.../kilometre`（除以 1000）。
- 结果 `xsd:double`；输入含 Rect 或非 CRS84 → 确定性错误。

### 4.2 `geof:envelope(?g)`

- 最小边界矩形：Point → 自身 Rect；Rect → 自身。
- 结果 `geo:wktLiteral`（`ENVELOPE (…)` 形式）。

### 4.3 `geof:getSRID(?g)`

- 返回 `xsd:integer` 4326（CRS84 的 EPSG 码）。

### 4.4 `geof:isSimple(?g)` / `geof:isValid(?g)`

- 范围几何恒为 true（`xsd:boolean`）。

### 4.5 拓扑函数 `geof:sf*`

| 函数 | Point–Point | Point–Rect | Rect–Rect |
|------|-------------|------------|-----------|
| sfEquals | 坐标全等 | 点在矩形内边界语义：仅当点与退化矩形全等 | 四边全等 |
| sfDisjoint | 坐标不同 | 点在矩形外 | 无交集（不含边界） |
| sfIntersects | 坐标相同 | 点在矩形内或边界 | 有交集（含边界） |
| sfTouches | 恒 false | 点在矩形边界上 | 仅边界接触、内部不相交 |
| sfCrosses | 恒 false | 恒 false（维数规则） | 恒 false |
| sfWithin | 坐标相同 | 点在矩形内（不含边界=strict） | 被包含（含边界） |
| sfContains | 坐标相同 | 矩形含点（含边界） | 包含（含边界） |
| sfOverlaps | 恒 false | 恒 false | 内部重叠（维数=2） |

拓扑按 DE-9IM 的 sf 维度规则在 Point/Rect 上精确实现；结果 `xsd:boolean`。

## 5. 属性函数（BGP 重写）

在基本图模式求值中，当谓词为以下 IRI 且 subject 已绑定为范围几何字面量时，
产生合成候选（不查询存储；存储中若显式存在同谓词三元组则照常匹配）：

- `geo:asWKT`：object ← 几何的 WKT 词法形式（plain string）。
- `geo:asGeoJSON`：object ← 几何的 GeoJSON 词法形式（plain string）。
- `geo:hasGeometry`：object ← subject 自身（几何字面量恒等映射）。

非几何/未绑定方向不产生绑定（与属性函数约定一致），不做隐式类型强制。

## 6. 确定性错误模型

函数参数类型不匹配、WKT/GeoJSON 语法错误、非 CRS84、非范围几何形状、未知
单位 → 该表达式返回错误（按 SPARQL 表达式错误传播语义，求值结果为
unbound，不使整个查询失败）；`/explain` 的规范化错误文案恒定。

## 7. 验收判据

- R3-01：WKT/GeoJSON 解析与序列化往返、非法输入错误文案确定（crate 单测）。
- R3-02：全部 `geof:`/`sf:` 函数行为与 §4 表一致（crate + SPARQL 层测试）。
- R3-03：`geo:asWKT`/`asGeoJSON`/`hasGeometry` 属性函数在 SPARQL 查询中
  可用且与存储三元组互不干扰。
- R3-04：`ontolith-compliance` `r3_geo_gate` 覆盖：确定性（同查询两次字节级
  一致）、拓扑表抽查、距离数值断言、属性函数接线；workspace 全量 +
  W3C 492/492 + SHACL 98/98 零漂移；clippy 零警告 + fmt 对齐。

## 8. 实现落点

- 新 crate `crates/ontolith-geo`（零外部依赖）：几何模型、WKT/GeoJSON 解析
  与序列化、haversine 距离、DE-9IM 拓扑。
- `ontolith-query`：`geof:`/`sf:` 函数经 `eval_function` 接线（函数名 IRI
  归一匹配）；`geo:` 属性函数经 `eval_bgp` 重写。
- `ontolith-compliance`：`r3_geo_gate` 端到端（SPARQL 层）。
- CI：`r3-gates` 作业（geo + ha + security 三合一，见 R3 台账）。

## 9. 引用

- [ADR-0005](../adr/0005-geosparql-scoped-capability.md)
- [SAS-0001 §3](./Ontolith_Software_Architecture_Specification.md)
- [PLAN-0001 §6 R3](./Ontolith_Development_Plan.zh-CN.md)
- OGC GeoSPARQL 1.1（范围剖面引用：WKT/GeoJSON 序列化、sf 拓扑、geof 函数 IRI）

## 10. 评审记录（2026-08-09）

- 评审人：sharky-ai（项目负责人；Codex 执行体代为执行评审流程）
- 范围：范围剖面（§2/§3）、函数语义表（§4）、属性函数重写（§5）、错误模型（§6）、
  验收判据（§7）逐项对照实现（`crates/ontolith-geo` + `ontolith-query` 接线 +
  `ontolith-compliance/tests/r3_geo_gate.rs`）核验一致。
- 结论：**通过，转 1.0.0 Approved**。证据：`r3_geo_gate` 5 测全绿（距离数值
  343–344.5km 区间、拓扑表 Point/Rect 全项、属性函数与存储回退、确定性字节级、
  错误 unbound 语义）；workspace 全量 + W3C 492/492 + SHACL 98/98 零漂移 +
  clippy 零警告 + fmt 对齐。
