审查完成。所有核对均通过读取仓库实际代码与运行测试完成，未修改任何文件。

---

## 代码审查：OWL 2 RL 键与一致性规则（c183759）

### 已验证事项（全部通过）

| 检查项 | 结果 |
|---|---|
| 全量测试 | 244 测全部通过（`cargo test --workspace`），其中 reasoner 41 测（mod.rs 20 + shacl.rs 21），与本波文档声称的 37→41 一致 |
| 数字一致性 | `#[test]` 实测 6c35ebc=10 → 64f1209=13 → 793aa5f=16 → 289e635=16 → c183759=20；"forward-chain 15→19"、"P6-01 19 测"与 mod.rs 实际 16→20 恒差 1（历史一致的排除约定，非本波引入） |
| 构建 | `cargo fmt --check` 通过、`cargo clippy -p ontolith-reasoner` 无告警 |
| `ReasoningReport` 新增字段 | 仅 domain 定义 + infra 两处构造点（mode Off 与正常路径）均已补齐，无遗漏构造点（结构体字面量无 `..default()`，编译器已兜底） |
| `Rule` 枚举 | 六个变体 + `as_str` 映射 + `supported_rules` 三处增补一致，`supported_rules_covers_extended_set` 测试覆盖 |
| 测试正确性 | `has_key_properties_infer_same_as` 的负断言（`!carol_alice`）有效；eq-sym 补充的 `bob_alice` 依赖迭代闭包，成立 |

### 发现

**F1 — [中] prp-key 单次迭代 O(m²·k·n)～O(m²·k·n²)，墙钟护栏无法中断单次 apply_rules**
`infrastructure/mod.rs:585-604`。`share_key` 对每对成员×每个键做两层全 closure 线性扫描；成员对循环为 O(m²)。`max_elapsed_ms` 只在迭代**之间**检查（`mod.rs:60-65`），单次 `apply_rules` 内超预算不会被提前打断。一个 hasKey 公理 + 大类的输入（m≈几千）即可让单次迭代显著超时。
建议：按 (class, key, value) 预建值→成员桶索引，两两取交集；或在 `apply_rules` 内注入预算检查点。注意当前 reasoner 尚未接入 server 管线（P6-03 的"接入"仍是下次动作），暂无线上暴露面。

**F2 — [低] eq-diff1/eq-diff2 对空白节点对象不可见**
`infrastructure/mod.rs:679-694`。`iri_of(&t.object)` 仅接受 `Term::Iri`，`owl:differentFrom` 对象为 bnode 时（含 `_:b differentFrom _:b`）被静默跳过；`same_as` 索引同样只收录 IRI 对象。与引擎既有 IRI 中心化索引风格一致，属可接受的不完备，但建议在注释中说明。另注意 `node_iri` 对 `_:` 前缀解码串可通过 `Iri::parse`（其校验仅要求含 `:`，见 `resource.rs:24-40`），因此 bnode **主语**以伪 IRI 形式参与 sameAs/subclass 索引——这与 F2 形成不对称：bnode 主语的 sameAs 对称/传递生效，bnode 对象的 differentFrom 冲突不生效。

**F3 — [低] 一致性检测存在同迭代滞后，`max_iterations=1` 时会漏报**
`infrastructure/mod.rs:636-694`。`subclass`/`disjoint`/`same_as` 索引均在 `apply_rules` 开头从当次迭代起点闭包构建；同一迭代内经 rdfs5（传递 subclass → Nothing）、cax-sco（派生 `x type Dog` 触发 cax-dw）、eq-sym（反方向 differentFrom 冲突）新派生出的三元组要等**下一迭代**才能被一致性规则看到。默认 16 次迭代下收敛正常（最后一次 `apply_rules` 即使触发 `new_count==0` 提前退出也已在全闭包上跑过，不会永久漏报），但把 `max_iterations` 作为受支持的护栏配置时，`=1` 会静默漏掉需要链式派生的 ⊥ 检测。建议在文档或注释中注明"一致性检测需 ≥2 次迭代"。

**F4 — [低] 测试缺口（负向与边界）**
新增 4 测均为正向断言，缺失：① 一致输入上 `inconsistent == false` 的负断言；② 单键 hasKey（最常见形态）；③ 键值为字面量（`LiteralValue` 按词法+datatype 比较，`"1"^^xsd:int` vs `"1"^^xsd:integer` 不共享，属潜在漏报，未见归一化层佐证）；④ 反方向 `y differentFrom x` + `x sameAs y`（需 eq-sym 迭代）；⑤ 传递 subClassOf 链到 Nothing；⑥ `list_members` 环保护在 hasKey 列表上的行为。

**F5 — [信息] 文档数字**：`PROGRESS.md` 各表（Phase 6 ~55%、L6 41 测、P6-01 19 测、变更日志 37→41/15→19/244 通过）与代码、测试结果一致；仅 "forward-chain" 与 mod.rs 实测 `#[test]` 恒差 1，系既有约定，可自行决定是否对齐。

### 结论
逻辑正确：prp-key 的"每键共享值"语义与 OWL 2 RL 规则体（每键共享同一 `?yi` 变量）一致；五个 ⊥ 检测触发条件与 cax-dw/cls-nothing1/cls-nothing2/eq-diff1/eq-diff2 规则一致，无发现误报路径；`inconsistent` 单调累积、在 `new_count==0` 提前退出路径上仍能正确置位。主要关注点是 F1 的性能放大（建议在接入 server 前处理）与 F3 的迭代数依赖（建议文档化）。无安全（注入/越界/逃逸）问题。

---

## 修复状态（2026-08-07 已全部落地）

| 编号 | 结论 | 修复 |
|------|------|------|
| F1 | 属实 | `prp-key` 改为按 (key, value) 预建成员桶索引后两两取交集，消除 O(m²·k·n²) 对扫；一致性/同迭代检测仅剩轻量扫描 |
| F2 | 属实 | `same_as` 索引改为 `(NodeId, NodeId)`，`eq-sym`/`eq-trans`/`eq-diff1`/`eq-diff2` 全面 bnode 感知 |
| F3 | 属实 | 一致性段改用 `closure ∪ frontier`（同迭代派生可见），`max_iterations=1` 也能检出链式 ⊥（rdfs5→cls-nothing2、eq-sym→eq-diff2 等） |
| F4 | 属实 | 补 6 测：一致输入负断言、单键 hasKey、字面量键值（Integer vs Decimal 不跨类型匹配）、反向 differentFrom、传递 subClassOf 链到 Nothing、hasKey 环列表终止 |
| F5 | 信息 | `PROGRESS.md` 数字与代码一致；forward-chain 与 mod.rs 恒差 1 系既有排除 `supported_rules` 测试的约定，保持不变 |

验证：reasoner 41→47 测全绿，全量测试 250 通过，`cargo fmt --check` / `clippy` 干净。
