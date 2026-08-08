# Ontolith Benchmarks

性能基线门禁（P7-02 / §6 质量门禁）。

## 运行

```bash
# 存储热路径微基准（字典编码 / 事务写入提交 / 索引匹配）
cargo bench -p ontolith-storage

# 阈值断言 + 趋势记录（推荐，CI 同款）
bash scripts/check-bench-thresholds.sh
```

## 当前基线（2026-08-06 建立，2026-08-08 阈值化）

`crates/ontolith-storage/benches/storage_bench.rs`（无第三方依赖，
`harness = false` + `std::time`）覆盖：

- `dict encode_node`：字典双向映射编码。
- `triple insert + commit`：事务 stage + commit 全链路。
- `match by subject (1k triples)`：多索引匹配读路径。

输出格式：`<case> <iterations> ops <ns/op> total <ms>`。

## 门禁目标

- 每次提交在 CI `bench` 作业执行冒烟基准并做**阈值断言**（硬门禁，超阈值即失败）。
- 每次运行追加一条 JSONL 趋势记录，供跨提交回归观察。

## 阈值与趋势

`scripts/check-bench-thresholds.sh` 执行三步：

1. 运行 `cargo bench -p ontolith-storage --bench storage_bench`。
2. 按 case 断言 `ns/op` 不超过阈值（默认值可经环境变量覆盖）：

   | case | 默认阈值（ns/op） | 覆盖变量 |
   |------|-------------------|----------|
   | `dict encode_node` | 5000 | `ONTOLITH_BENCH_DICT_MAX_NS` |
   | `triple insert + commit` | 20000 | `ONTOLITH_BENCH_INSERT_MAX_NS` |
   | `match by subject (1k triples)` | 5000000 | `ONTOLITH_BENCH_MATCH_MAX_NS` |

3. 向趋势文件追加 JSONL（每 case 一行）：

   ```json
   {"run_id":"<git short sha>","case":"dict encode_node","iterations":20000,"per_op_ns":142,"total_ms":2.84}
   ```

趋势文件默认 `benchmarks/trends/storage-bench.jsonl`（可用
`ONTOLITH_BENCH_TREND_PATH` 覆盖；`ONTOLITH_BENCH_RUN_ID` 覆盖 run_id）。

实测基线（2026-08-08，本机）：`dict` ≈ 142 ns/op、`insert` ≈ 630 ns/op、
`match` ≈ 722k ns/op——远低于阈值；阈值是宽松回归护栏而非优化目标。
