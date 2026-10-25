//! 纯内存向量索引（D35 / §6.3）。
//!
//! v1 **不引第三方 ANN**：每个 `(robot_id, collection)` 一份连续向量数组，
//! 查询线性扫描。没有图结构、没有磁盘索引文件，进程重启后从 PG 重建。
//!
//! 分数一律是**相似度**（越大越近，D41）：`sim = 1 - cos_distance = cos_similarity`。
//! A 里 QA / 文档检索用的是 `distance <= recall_distance`，方向相反；统一入口见
//! [`is_hit`]（B-31）。
//!
//! 精度（B-27）：默认 f32；一份索引超过内存预算时降成 **f16**（`Precision::F16`），
//! 内存减半、余弦相似度误差在 1e-3 量级。小于 `2^-14` 的分量编码时刷成 0 ——
//! 对归一化后的句向量没有影响，却省掉 f16 subnormal 的边界复杂度。
//! 再不够就轮到 ANN（C 阶段，D35），v1 不做。
//!
//! 分派用 `enum IndexBackend` 而不是 `dyn VectorIndex`：C 阶段接 ES8 / Milvus 时
//! 只需要加一个枚举成员。trait 本身用**同步**方法 —— v1 只有本地内存后端，异步
//! 签名只会让每个调用点多一层 `await`；远程后端在枚举这一层包异步即可
//! （D35 里"trait 用 async fn"的动机是给远程后端留口，位置在这里）。

use std::collections::HashMap;

use crate::result::{Error, Result};

pub(crate) type ItemId = u64;

/// 一条待写入的向量：`id` 是 `vector_items.id`。
pub(crate) struct VectorItem {
    pub(crate) id: ItemId,
    pub(crate) vec: Vec<f32>,
}

/// 一条命中：`score` 是相似度（越大越近）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScoredId {
    pub(crate) id: ItemId,
    pub(crate) score: f32,
}

/// 索引的规模与内存占用（诊断 / 测试 / 预算）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IndexStats {
    pub(crate) len: usize,
    pub(crate) dimension: usize,
    /// 估算的常驻字节数（向量数据 + id + 范数 + 槽位）。
    pub(crate) bytes: usize,
    /// 内存里用的分量精度（B-27）。
    pub(crate) precision: Precision,
}

/// 内存里的分量精度。f16 只用于预算不够时（B-27）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Precision {
    F32,
    F16,
}

impl Precision {
    pub(crate) fn bytes_per_component(self) -> usize {
        match self {
            Precision::F32 => 4,
            Precision::F16 => 2,
        }
    }
}

/// 一个 `(robot_id, collection)` 的索引。方法都是同步的，见模块注释。
pub(crate) trait VectorIndex: Send {
    fn upsert(&mut self, items: &[VectorItem]) -> Result<()>;
    fn delete(&mut self, ids: &[ItemId]) -> Result<()>;
    fn search(&self, query: &[f32], limit: usize) -> Result<Vec<ScoredId>>;
    fn clear(&mut self) -> Result<()>;
    fn stats(&self) -> Result<IndexStats>;
}

/// 相似度是否达到阈值（D41 / B-31）。
///
/// 三处调用点（意图、QA、文档）都必须走这里，避免再出现"一处比距离、一处比相似度"
/// 的历史问题。阈值语义：**越大越严格**（`1.0` = 只有完全相同才命中）。
pub(crate) fn is_hit(similarity: f32, threshold: f32) -> bool {
    similarity >= threshold
}

/// 余弦相似度；任一向量范数为 0（空 / 全零）时返回 0。
///
/// 不能用"单位向量直接点积"：本地 HF 路径输出已经归一化，但 OpenAI 兼容端点
/// 不保证（很多网关原样转发模型的未归一化输出）。A 的 `vector_distance_cos` 自己
/// 归一化，所以这里也必须归一化，否则同一份数据在换库前后排序会变。
///
/// 生产路径用的是扁平行 + 预计算范数（见 [`BruteForce::search`]）；这个函数是它的
/// 参考实现，只被单测用来钉住方向语义（D41）。
#[cfg(test)]
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= f32::EPSILON || nb <= f32::EPSILON {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// 向量 → `BYTEA`：小端 f32 拼接。`vector_items.vec` 就是这个格式。
pub(crate) fn encode(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// `BYTEA` → 向量。长度不是 4 的倍数时按坏数据返回错误（不在检索路径上 panic）。
pub(crate) fn decode(b: &[u8]) -> Result<Vec<f32>> {
    if !b.len().is_multiple_of(4) {
        return Err(Error::WithMessage(format!(
            "Corrupted vector blob: {} bytes is not a multiple of 4",
            b.len()
        )));
    }
    let mut out = Vec::with_capacity(b.len() / 4);
    for chunk in b.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(*chunk));
    }
    Ok(out)
}

// --------------------------------------------------------------------------- f16

/// f32 → IEEE 754 binary16 的位模式（round-to-nearest-even）。
///
/// 只用于**内存索引**的降精度存储，不写回 PG（`vector_items.vec` 仍是 f32）。
/// `2^-14` 以下的分量刷成 0（embeddings 里这些值对余弦相似度没有可见影响）。
pub(crate) fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x007f_ffff;
    if exp == 0xff {
        // inf / NaN
        let m = if mant != 0 { 0x0200 } else { 0 };
        return sign | 0x7c00 | m;
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00; // 溢出 → inf
    }
    if e <= 0 {
        return sign; // 下溢（含 0）→ ±0
    }
    let mut m = (mant >> 13) as u16;
    let rem = mant & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && (m & 1) == 1) {
        m += 1;
        if m == 0x0400 {
            // 尾数进位到指数
            if e + 1 >= 0x1f {
                return sign | 0x7c00;
            }
            return sign | (((e + 1) as u16) << 10);
        }
    }
    sign | ((e as u16) << 10) | m
}

/// binary16 位模式 → f32。保留 subnormal 解码（虽然编码器不会产出）。
pub(crate) fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 1;
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x03ff) as f32;
    let out = if exp == 0 {
        mant * 2f32.powi(-24)
    } else if exp == 0x1f {
        if mant == 0.0 { f32::INFINITY } else { f32::NAN }
    } else {
        (1.0 + mant / 1024.0) * 2f32.powi(exp - 15)
    };
    if sign == 1 { -out } else { out }
}

/// 一份索引的估算字节数：向量数据 + 每行的 id/范数/槽位开销。
///
/// 用于**加载前**决定精度与是否需要淘汰；建好之后以 [`BruteForce::bytes`]
/// 的实际值为准（两者用同一个公式，测试里对齐）。
pub(crate) fn estimated_bytes(rows: usize, dim: usize, precision: Precision) -> usize {
    rows * dim * precision.bytes_per_component() + rows * ROW_OVERHEAD_BYTES
}

/// 每行的 id(8) + 范数(4) + HashMap 槽位估算(24)。
const ROW_OVERHEAD_BYTES: usize = 8 + 4 + 24;

/// 预算不够时选精度：能放下 f32 就用 f32，否则 f16。
pub(crate) fn choose_precision(rows: usize, dim: usize, budget: usize) -> Precision {
    if estimated_bytes(rows, dim, Precision::F32) <= budget {
        Precision::F32
    } else {
        Precision::F16
    }
}

// --------------------------------------------------------------------------- 存储

/// 扁平行主序的向量数据，按精度选择底层数组。
enum Data {
    F32(Vec<f32>),
    /// f16 的位模式（2 字节），解码见 [`f16_to_f32`]。
    F16(Vec<u16>),
}

impl Data {
    fn new(precision: Precision) -> Self {
        match precision {
            Precision::F32 => Data::F32(Vec::new()),
            Precision::F16 => Data::F16(Vec::new()),
        }
    }

    fn len(&self) -> usize {
        match self {
            Data::F32(v) => v.len(),
            Data::F16(v) => v.len(),
        }
    }

    fn bytes(&self) -> usize {
        self.len() * self.bytes_per_component()
    }

    fn bytes_per_component(&self) -> usize {
        match self {
            Data::F32(_) => 4,
            Data::F16(_) => 2,
        }
    }

    fn push(&mut self, v: &[f32]) {
        match self {
            Data::F32(d) => d.extend_from_slice(v),
            Data::F16(d) => d.extend(v.iter().map(|x| f32_to_f16(*x))),
        }
    }

    fn set(&mut self, slot: usize, dim: usize, v: &[f32]) {
        match self {
            Data::F32(d) => d[slot * dim..(slot + 1) * dim].copy_from_slice(v),
            Data::F16(d) => {
                for (i, x) in v.iter().enumerate() {
                    d[slot * dim + i] = f32_to_f16(*x);
                }
            }
        }
    }

    fn get(&self, i: usize) -> f32 {
        match self {
            Data::F32(d) => d[i],
            Data::F16(d) => f16_to_f32(d[i]),
        }
    }

    /// 第 `slot` 行的点积（query 的长度必须等于 `dim`）。
    fn dot(&self, slot: usize, dim: usize, query: &[f32]) -> f32 {
        let base = slot * dim;
        let mut dot = 0.0f32;
        for (d, q) in query.iter().enumerate() {
            dot += self.get(base + d) * q;
        }
        dot
    }

    /// 第 `slot` 行的范数（按**存下来的**分量算，f16 降精度后仍自洽）。
    fn norm(&self, slot: usize, dim: usize) -> f32 {
        let base = slot * dim;
        let mut sq = 0.0f32;
        for d in 0..dim {
            let x = self.get(base + d);
            sq += x * x;
        }
        sq.sqrt()
    }

    fn swap_row(&mut self, a: usize, b: usize, dim: usize) {
        match self {
            Data::F32(d) => {
                for k in 0..dim {
                    d.swap(a * dim + k, b * dim + k);
                }
            }
            Data::F16(d) => {
                for k in 0..dim {
                    d.swap(a * dim + k, b * dim + k);
                }
            }
        }
    }

    fn truncate(&mut self, components: usize) {
        match self {
            Data::F32(d) => d.truncate(components),
            Data::F16(d) => d.truncate(components),
        }
    }

    fn clear(&mut self) {
        match self {
            Data::F32(d) => d.clear(),
            Data::F16(d) => d.clear(),
        }
    }
}

/// v1 的索引实现：连续向量数组 + 线性扫描。
///
/// 内存 = `N × dim × (4 或 2)B` + 每行约 36B 的 id/范数/槽位开销。
/// 10 万条 × 1024 维 f32 ≈ 400 MB，f16 ≈ 204 MB —— 因此 store 层按
/// `(robot, collection)` 懒加载、按预算淘汰、超预算时降精度（B-27）。
pub(crate) struct BruteForce {
    dim: usize,
    precision: Precision,
    ids: Vec<ItemId>,
    data: Data,
    /// 预计算范数（由**存储中的分量**算出），避免每次查询重复开方。
    norms: Vec<f32>,
    slots: HashMap<ItemId, usize>,
}

impl BruteForce {
    pub(crate) fn new(dim: usize, precision: Precision) -> Self {
        Self {
            dim,
            precision,
            ids: Vec::new(),
            data: Data::new(precision),
            norms: Vec::new(),
            slots: HashMap::new(),
        }
    }

    /// 从 `(vector_items.id, vec)` 建索引（f32 精度）。测试辅助。
    #[cfg(test)]
    pub(crate) fn from_items(items: Vec<(ItemId, Vec<f32>)>) -> Result<Self> {
        Self::from_items_with(items, Precision::F32)
    }

    /// 从 `(vector_items.id, vec)` 建索引，指定精度。测试辅助。
    #[cfg(test)]
    pub(crate) fn from_items_with(
        items: Vec<(ItemId, Vec<f32>)>,
        precision: Precision,
    ) -> Result<Self> {
        let dim = items.first().map(|(_, v)| v.len()).unwrap_or(0);
        let mut idx = Self::new(dim, precision);
        for (id, vec) in items {
            idx.insert(id, &vec)?;
        }
        Ok(idx)
    }

    /// 插入或覆盖一条向量（`from_items` / 流式加载 / 写后同步共用）。
    pub(crate) fn insert(&mut self, id: ItemId, vec: &[f32]) -> Result<()> {
        if self.dim == 0 {
            self.dim = vec.len();
        }
        if vec.len() != self.dim {
            // 混合维度是真问题（换维度却没重建），但这里不是报错的地方：
            // 索引是缓存，调用方会把整份缓存丢掉重载，由 PG 决定事实。
            return Err(Error::WithMessage(format!(
                "Vector dimension mismatch: index has {} dims, item has {}",
                self.dim,
                vec.len()
            )));
        }
        match self.slots.get(&id).copied() {
            Some(slot) => {
                self.data.set(slot, self.dim, vec);
                self.norms[slot] = self.data.norm(slot, self.dim);
            }
            None => {
                let slot = self.ids.len();
                self.ids.push(id);
                self.data.push(vec);
                self.norms.push(self.data.norm(slot, self.dim));
                self.slots.insert(id, slot);
            }
        }
        Ok(())
    }

    /// 估算的常驻字节数。
    pub(crate) fn bytes(&self) -> usize {
        self.data.bytes() + self.ids.len() * 8 + self.norms.len() * 4 + self.slots.len() * 24
    }

    /// 把第 `slot` 条挪走（与最后一条交换），并维护 `slots`。
    fn swap_remove(&mut self, slot: usize) {
        let last = self.ids.len() - 1;
        if slot != last {
            self.ids.swap(slot, last);
            self.data.swap_row(slot, last, self.dim);
            self.norms.swap(slot, last);
            let moved = self.ids[slot];
            self.slots.insert(moved, slot);
        }
        self.ids.pop();
        self.data.truncate(last * self.dim);
        self.norms.pop();
    }
}

impl VectorIndex for BruteForce {
    fn upsert(&mut self, items: &[VectorItem]) -> Result<()> {
        for item in items {
            self.insert(item.id, &item.vec)?;
        }
        Ok(())
    }

    fn delete(&mut self, ids: &[ItemId]) -> Result<()> {
        for id in ids {
            if let Some(slot) = self.slots.remove(id) {
                self.swap_remove(slot);
            }
        }
        Ok(())
    }

    fn search(&self, query: &[f32], limit: usize) -> Result<Vec<ScoredId>> {
        if limit == 0 || self.ids.is_empty() {
            return Ok(Vec::new());
        }
        if query.len() != self.dim {
            return Err(Error::WithMessage(format!(
                "Query dimension mismatch: index has {} dims, query has {}",
                self.dim,
                query.len()
            )));
        }
        let qn = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        if qn <= f32::EPSILON {
            return Ok(Vec::new());
        }
        let mut scored: Vec<ScoredId> = Vec::with_capacity(self.ids.len());
        for (i, id) in self.ids.iter().enumerate() {
            let denom = self.norms[i] * qn;
            let score = if denom <= f32::EPSILON {
                0.0
            } else {
                self.data.dot(i, self.dim, query) / denom
            };
            scored.push(ScoredId { id: *id, score });
        }
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    fn clear(&mut self) -> Result<()> {
        self.ids.clear();
        self.data.clear();
        self.norms.clear();
        self.slots.clear();
        Ok(())
    }

    fn stats(&self) -> Result<IndexStats> {
        Ok(IndexStats {
            len: self.ids.len(),
            dimension: self.dim,
            bytes: self.bytes(),
            precision: self.precision,
        })
    }
}

/// 索引后端。v1 只有一个成员；C 阶段的 ES8 / Milvus 加在这里。
pub(crate) enum IndexBackend {
    BruteForce(BruteForce),
}

impl VectorIndex for IndexBackend {
    fn upsert(&mut self, items: &[VectorItem]) -> Result<()> {
        match self {
            IndexBackend::BruteForce(i) => i.upsert(items),
        }
    }

    fn delete(&mut self, ids: &[ItemId]) -> Result<()> {
        match self {
            IndexBackend::BruteForce(i) => i.delete(ids),
        }
    }

    fn search(&self, query: &[f32], limit: usize) -> Result<Vec<ScoredId>> {
        match self {
            IndexBackend::BruteForce(i) => i.search(query, limit),
        }
    }

    fn clear(&mut self) -> Result<()> {
        match self {
            IndexBackend::BruteForce(i) => i.clear(),
        }
    }

    fn stats(&self) -> Result<IndexStats> {
        match self {
            IndexBackend::BruteForce(i) => i.stats(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[f32]) -> Vec<f32> {
        xs.to_vec()
    }

    fn ids(hits: &[ScoredId]) -> Vec<ItemId> {
        hits.iter().map(|h| h.id).collect()
    }

    #[test]
    fn cosine_is_similarity_not_distance() {
        // D41：1 = 同向，0 = 正交，-1 = 反向。旧代码里的 distance = 1 - 这个值。
        assert!((cosine(&v(&[1.0, 0.0]), &v(&[1.0, 0.0])) - 1.0).abs() < 1e-6);
        assert!(cosine(&v(&[1.0, 0.0]), &v(&[0.0, 1.0])).abs() < 1e-6);
        assert!((cosine(&v(&[1.0, 0.0]), &v(&[-1.0, 0.0])) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_normalizes_unnormalized_inputs() {
        // 同一个方向、不同长度 → 相似度仍然是 1（A 的 vector_distance_cos 也这样）。
        assert!((cosine(&v(&[3.0, 4.0]), &v(&[30.0, 40.0])) - 1.0).abs() < 1e-6);
        // 零向量没有方向，不能除出 NaN。
        assert_eq!(cosine(&v(&[0.0, 0.0]), &v(&[1.0, 1.0])), 0.0);
    }

    #[test]
    fn threshold_is_a_similarity_threshold() {
        // 0.9 = 很严；0.1 = 很松。方向不同（距离语义）时这两句会恰好反过来。
        assert!(is_hit(0.95, 0.9));
        assert!(!is_hit(0.85, 0.9));
        assert!(is_hit(0.5, 0.1));
        assert!(!is_hit(-0.5, 0.1));
        // 正好等于阈值算命中，与 A 的 `distance <= recall_distance` 边界一致。
        assert!(is_hit(0.9, 0.9));
    }

    #[test]
    fn encode_decode_roundtrip() {
        let vec = vec![0.0f32, 1.5, -2.25, 1e-9];
        let blob = encode(&vec);
        assert_eq!(blob.len(), 16);
        assert_eq!(decode(&blob).unwrap(), vec);
        // 坏长度不 panic，返回错误。
        assert!(decode(&[1, 2, 3]).is_err());
        assert!(decode(&[]).unwrap().is_empty());
    }

    /// f16 编码器只用于内存索引（B-27），必须和 IEEE binary16 对齐。
    #[test]
    fn f16_matches_known_values() {
        assert_eq!(f32_to_f16(0.0), 0x0000);
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
        assert_eq!(f32_to_f16(65504.0), 0x7bff);
        assert_eq!(f32_to_f16(100000.0), 0x7c00, "溢出要变成 inf");
        assert!(f32_to_f16(f32::INFINITY) == 0x7c00);
        // 1.0 与 -1.0 的往返是精确的；0.1 只能到 f16 的分辨率。
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert!((f16_to_f32(f32_to_f16(0.1)) - 0.1).abs() < 1e-3);
        // 下溢刷成 0（embeddings 里远小于 2^-14 的分量没有意义）。
        assert_eq!(f32_to_f16(1e-6), 0x0000);
        assert_eq!(f16_to_f32(0x0000), 0.0);
    }

    #[test]
    fn search_returns_best_first_with_similarity_scores() {
        let idx = BruteForce::from_items(vec![
            (1, v(&[1.0, 0.0])),
            (2, v(&[0.8, 0.6])),
            (3, v(&[0.0, 1.0])),
        ])
        .unwrap();
        let hits = idx.search(&v(&[1.0, 0.0]), 2).unwrap();
        assert_eq!(ids(&hits), vec![1, 2], "越大越近，必须是降序");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        assert!((hits[1].score - 0.8).abs() < 1e-6);
    }

    #[test]
    fn upsert_replaces_instead_of_duplicating() {
        let mut idx = BruteForce::from_items(vec![(1, v(&[1.0, 0.0]))]).unwrap();
        idx.upsert(&[VectorItem {
            id: 1,
            vec: v(&[0.0, 1.0]),
        }])
        .unwrap();
        assert_eq!(idx.stats().unwrap().len, 1);
        let hits = idx.search(&v(&[0.0, 1.0]), 10).unwrap();
        assert_eq!(ids(&hits), vec![1]);
    }

    #[test]
    fn delete_swaps_rows_without_corrupting_neighbours() {
        let mut idx = BruteForce::from_items(vec![
            (1, v(&[1.0, 0.0])),
            (2, v(&[0.0, 1.0])),
            (3, v(&[1.0, 1.0])),
        ])
        .unwrap();
        idx.delete(&[1]).unwrap();
        assert_eq!(idx.stats().unwrap().len, 2);
        // 2 和 3 仍然各自是原来那条向量：交换删除不能把数据搬错。
        let hits = idx.search(&v(&[0.0, 1.0]), 2).unwrap();
        assert_eq!(ids(&hits), vec![2, 3]);
        idx.delete(&[2, 3]).unwrap();
        assert_eq!(idx.stats().unwrap().len, 0);
        assert!(idx.search(&v(&[1.0, 0.0]), 5).unwrap().is_empty());
    }

    #[test]
    fn dimension_mismatch_is_an_error_not_a_panic() {
        let mut idx = BruteForce::from_items(vec![(1, v(&[1.0, 0.0]))]).unwrap();
        assert!(
            idx.upsert(&[VectorItem {
                id: 2,
                vec: v(&[1.0, 0.0, 0.0])
            }])
            .is_err()
        );
        assert!(idx.search(&v(&[1.0, 0.0, 0.0]), 1).is_err());
    }

    #[test]
    fn clear_empties_the_index_but_keeps_the_dimension() {
        let mut idx = BruteForce::from_items(vec![(1, v(&[1.0, 0.0]))]).unwrap();
        idx.clear().unwrap();
        let s = idx.stats().unwrap();
        assert_eq!(s.len, 0);
        assert_eq!(s.dimension, 2);
        assert!(idx.search(&v(&[1.0, 0.0]), 1).unwrap().is_empty());
    }

    /// B-27 的核心：精度降级后内存明显变小，命中结果不变（误差在 1e-3 量级）。
    #[test]
    fn f16_index_ranks_like_f32_with_less_memory() {
        /// 确定性、两两可区分的 128 维向量（伪随机，互相近似正交）。
        fn item(i: u64) -> Vec<f32> {
            let mut state = i
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (0..128u64)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((state >> 33) as f32) / ((1u64 << 31) as f32) - 1.0
                })
                .collect()
        }
        let items: Vec<(ItemId, Vec<f32>)> = (0..1000u64).map(|i| (i + 1, item(i))).collect();
        let f32_idx = BruteForce::from_items_with(items.clone(), Precision::F32).unwrap();
        let f16_idx = BruteForce::from_items_with(items, Precision::F16).unwrap();
        assert!(
            f16_idx.bytes() * 3 <= f32_idx.bytes() * 2,
            "f16 至少省三分之一：{} vs {}",
            f16_idx.bytes(),
            f32_idx.bytes()
        );
        // 每条向量自己当查询：降精度不能让它输给别人，自相似度仍接近 1。
        for i in 0..50u64 {
            let q = item(i);
            let id = i + 1;
            assert_eq!(f32_idx.search(&q, 1).unwrap()[0].id, id);
            let hit = f16_idx.search(&q, 1).unwrap()[0];
            assert_eq!(hit.id, id, "f16 下自查询必须仍然命中自己（第 {i} 条）");
            assert!(
                (hit.score - 1.0).abs() < 1e-3,
                "自相似度应该约等于 1：{}",
                hit.score
            );
        }
    }

    /// 预算与精度选择的数学要和实际占用一致（测试用同一公式对齐）。
    #[test]
    fn estimated_bytes_match_the_built_index() {
        let items: Vec<(ItemId, Vec<f32>)> = (0..10u64).map(|i| (i + 1, vec![0.5f32; 8])).collect();
        for precision in [Precision::F32, Precision::F16] {
            let idx = BruteForce::from_items_with(items.clone(), precision).unwrap();
            let actual = idx.stats().unwrap().bytes;
            let estimated = estimated_bytes(10, 8, precision);
            assert!(
                actual <= estimated && estimated - actual <= 10 * 32,
                "{precision:?}: 估算 {estimated} 与实际 {actual} 差太多"
            );
        }
    }

    #[test]
    fn precision_falls_back_to_f16_only_when_the_budget_requires_it() {
        // 100 行 × 8 维：f32 ≈ 100*8*4 + 3600 = 6800 B。
        assert_eq!(choose_precision(100, 8, 1_000_000), Precision::F32);
        assert_eq!(choose_precision(100, 8, 1000), Precision::F16);
        // 空集合任何时候都是 f32（不占内存）。
        assert_eq!(choose_precision(0, 0, 0), Precision::F32);
    }
}
