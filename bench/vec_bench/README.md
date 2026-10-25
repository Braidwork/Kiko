# vec_bench —— 100 万 × 1024 维向量索引压测

用于补齐 `doc/kiko-plan.md` §15.4 S4 验收里的「100 万合成向量的 P95 / 内存 /
冷启动数据」（B-27 / B-25 的决策依据）。**不是** v1 的运行时依赖。

它通过 `#[path = "../../../src/vector/index.rs"]` 直接引入进程内索引模块，
不依赖 Kiko 主 crate（也就不会把 candle 拉进来），编译几秒完成。

```text
cargo run --release -- f32 30          # 1,000,000 × 1024，f32，30 次查询
cargo run --release -- f16 30          # 同上，f16 精度
cargo run --release -- f32 30 100000   # 第三个参数可改行数（10 万）
```

输出：

- `build_ms`：从 `(id, vec)` 构造索引（冷启动的索引构建部分；不含 PG 读取与
  BYTEA 解码）；
- `accounted_bytes`：`BruteForce::bytes()` 的记账值；
- `p50_ms` / `p95_ms` / `max_ms` / `qps`：单次 top-10 查询的延迟；
- RSS 峰值用外部采样（Windows）：

```powershell
$p = Start-Process .\target\release\vec_bench.exe -ArgumentList 'f32','1' -PassThru -NoNewWindow
while (-not $p.HasExited) { (Get-Process -Id $p.Id).PeakWorkingSet64; Start-Sleep -Milliseconds 200 }
```

结果表与结论见 `doc/kiko-plan.md` §18 第九片。
