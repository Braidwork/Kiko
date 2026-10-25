//! 100 万 × 1024 维合成向量压测（B-27 / §15.4 S4 验收）。
//!
//! 只测量**索引本身**：构造（冷启动的索引构建部分）+ 线性扫描查询 P50/P95 + 占用。
//! 刻意不依赖 Kiko 主 crate（`#[path]` 直接引入 `src/vector/index.rs`），所以编译只要
//! 几秒；PG 读取与 BYTEA 解码不在测量范围内（那部分与维度无关，约 4 GB 顺序 I/O）。
//!
//! 用法：
//! ```text
//! cargo run --release -- f32 30     # f32 精度，30 次查询
//! cargo run --release -- f16 30
//! ```
//! 外部用 `PeakWorkingSet64` 采样进程峰值内存（Windows）。

#[path = "../../../src/vector/index.rs"]
mod index;

mod result {
    pub(crate) type Result<T> = std::result::Result<T, Error>;

    #[derive(Debug)]
    pub(crate) enum Error {
        WithMessage(String),
    }
}

use std::time::Instant;

use index::VectorIndex;

const ROWS: usize = 1_000_000;
const DIM: usize = 1024;
/// xorshift64*：确定性、无外部依赖。
struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        ((x >> 40) as f32) / ((1u64 << 24) as f32) - 1.0
    }

    fn fill_normalized(&mut self, v: &mut [f32]) {
        for x in v.iter_mut() {
            *x = self.next_f32();
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let precision = match args.get(1).map(String::as_str) {
        Some("f16") => index::Precision::F16,
        _ => index::Precision::F32,
    };
    let queries: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let rows: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(ROWS);
    println!(
        "mode={:?} rows={rows} dim={DIM} queries={queries} pid={}",
        precision,
        std::process::id()
    );

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut v = vec![0.0f32; DIM];
    let mut idx = index::BruteForce::new(DIM, precision);

    // 冷启动的索引构建部分：逐条生成 → 归一化 → 插入（不保留全部输入）。
    let build_start = Instant::now();
    for id in 0..rows as u64 {
        rng.fill_normalized(&mut v);
        idx.insert(id + 1, &v).unwrap();
    }
    let build = build_start.elapsed();
    let stats = idx.stats().unwrap();
    println!(
        "build_ms={} accounted_bytes={} rss_note=external",
        build.as_millis(),
        stats.bytes
    );
    flush();

    // 查询：每次换一个随机方向，取 top-10。
    let mut latencies: Vec<f64> = Vec::with_capacity(queries);
    let mut top_score = 0.0f32;
    for _ in 0..queries {
        rng.fill_normalized(&mut v);
        let t = Instant::now();
        let hits = idx.search(&v, 10).unwrap();
        latencies.push(t.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(hits.len(), 10);
        top_score += hits[0].score;
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let sum: f64 = latencies.iter().sum();
    println!(
        "p50_ms={:.1} p95_ms={:.1} max_ms={:.1} qps={:.2} avg_top_score={:.4}",
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.95),
        latencies.last().copied().unwrap_or(0.0),
        queries as f64 / sum * 1000.0,
        top_score / queries as f32
    );
    flush();
    // 让外部能在退出前采样；按 enter 退出（自动化时直接 EOF）。
    println!("done");
}

fn flush() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
}
