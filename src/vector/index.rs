//! 纯内存向量索引（D35 / §6.3）。
//!
//! v1 **不引第三方 ANN**：每个 `(robot_id, collection)` 一份连续 `Vec<f32>`，
//! 查询线性扫描。没有图结构、没有磁盘索引文件，进程重启后从 PG 重建。
//!
//! 分数一律是**相似度**（越大越近，D41）：`sim = 1 - cos_distance = cos_similarity`。
//! A 里 QA / 文档检索用的是 `distance <= recall_distance`，方向相反；统一入口见
//! [`is_hit`]（B-31）。
//!
//! 分派用 `enum IndexBackend` 而不是 `dyn VectorIndex`：C 阶段接 ES8 / Milvus 时
//! 只需要加一个枚举成员。trait 本身用**同步**方法 —— v1 只有本地内存后端，异步
//! 签名只会让每个调用点多一层 `await`；远程后端接入时在枚举这一层包异步即可
//! （D35 里"trait 用 async fn"的动机是给远程后端留口，位置在这里）。
//!
//! `ItemId` 来自 `vector_items.id`（D7：ANN 行 ID 与业务 ID 解耦）。

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

/// 索引的规模（诊断 / 测试）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IndexStats {
    pub(crate) len: usize,
    pub(crate) dimension: usize,
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

/// v1 的索引实现：连续 `Vec<f32>` + 线性扫描。
///
/// 内存 = `N × dim × 4B`（另有 `N × 4B` 的范数与 id/槽位开销）。10 万条 × 1024 维
/// ≈ 400 MB —— 因此 store 层按 `(robot, collection)` 懒加载并限制常驻数量
/// （见 `vector::mod`）。
pub(crate) struct BruteForce {
    dim: usize,
    ids: Vec<ItemId>,
    /// 扁平行主序：第 i 条向量是 `data[i * dim .. (i + 1) * dim]`。
    data: Vec<f32>,
    /// 预计算范数，避免每次查询重复开方。
    norms: Vec<f32>,
    slots: HashMap<ItemId, usize>,
}

impl BruteForce {
    pub(crate) fn new(dim: usize) -> Self {
        Self {
            dim,
            ids: Vec::new(),
            data: Vec::new(),
            norms: Vec::new(),
            slots: HashMap::new(),
        }
    }

    /// 从 `(vector_items.id, vec)` 建索引；空集合允许任意维度（第一次 upsert 定下来）。
    pub(crate) fn from_items(items: Vec<(ItemId, Vec<f32>)>) -> Result<Self> {
        let dim = items.first().map(|(_, v)| v.len()).unwrap_or(0);
        let mut idx = Self::new(dim);
        let items: Vec<VectorItem> = items
            .into_iter()
            .map(|(id, vec)| VectorItem { id, vec })
            .collect();
        idx.upsert(&items)?;
        Ok(idx)
    }

    fn push(&mut self, id: ItemId, vec: &[f32]) {
        let slot = self.ids.len();
        self.ids.push(id);
        self.data.extend_from_slice(vec);
        self.norms
            .push(vec.iter().map(|x| x * x).sum::<f32>().sqrt());
        self.slots.insert(id, slot);
    }

    /// 把第 `slot` 条挪走（与最后一条交换），并维护 `slots`。
    fn swap_remove(&mut self, slot: usize) {
        let last = self.ids.len() - 1;
        if slot != last {
            self.ids.swap(slot, last);
            for d in 0..self.dim {
                self.data.swap(slot * self.dim + d, last * self.dim + d);
            }
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
            if self.dim == 0 {
                self.dim = item.vec.len();
            }
            if item.vec.len() != self.dim {
                // 混合维度是真问题（换维度却没重建），但这里不是报错的地方：
                // 索引是缓存，调用方会把整份缓存丢掉重载，由 PG 决定事实。
                return Err(Error::WithMessage(format!(
                    "Vector dimension mismatch: index has {} dims, item has {}",
                    self.dim,
                    item.vec.len()
                )));
            }
            match self.slots.get(&item.id).copied() {
                Some(slot) => {
                    self.data[slot * self.dim..(slot + 1) * self.dim].copy_from_slice(&item.vec);
                    self.norms[slot] = item.vec.iter().map(|x| x * x).sum::<f32>().sqrt();
                }
                None => self.push(item.id, &item.vec),
            }
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
            let row = &self.data[i * self.dim..(i + 1) * self.dim];
            let dot: f32 = row
                .iter()
                .zip(query.iter())
                .map(|(a, b)| a * b)
                .sum::<f32>();
            let denom = self.norms[i] * qn;
            let score = if denom <= f32::EPSILON {
                0.0
            } else {
                dot / denom
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
}
