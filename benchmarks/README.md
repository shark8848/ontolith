# Ontolith Benchmarks

性能基线门禁（P7-02 / §6 质量门禁）。

## 运行

```bash
# 存储热路径微基准（字典编码 / 事务写入提交 / 索引匹配）
cargo bench -p ontolith-storage
```

## 当前基线（2026-08-06 建立）

`crates/ontolith-storage/benches/storage_bench.rs`（无第三方依赖，
`harness = false` + `std::time`）覆盖：

- `dict encode_node`：字典双向映射编码。
- `triple insert + commit`：事务 stage + commit 全链路。
- `match by subject (1k triples)`：多索引匹配读路径。

输出格式：`<case> <iterations> ops <ns/op> total <ms>`。

## 门禁目标

- 每次提交在 CI `bench` 作业执行冒烟基准，防回归。
- 后续接入阈值断言（如 P95 / 相对基线漂移）后转为硬门禁。
