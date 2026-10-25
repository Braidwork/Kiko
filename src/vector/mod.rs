//! 进程内向量索引的**注册表**：按 `(robot_id, collection)` 懒加载、写后同步、
//! 换 generation 后作废（D35 / D47 / §6.3）。
//!
//! 三条硬规矩：
//!
//! 1. **PG 是事实，索引是缓存**（D4）。索引永远从 `vector_items` 重建，不落磁盘；
//!    进程重启、加载失败、维度不一致，都只需要"丢掉缓存、下次从 PG 重读"。
//! 2. **先写 PG，后更新索引**。写完提交成功才调 [`upsert`] / [`delete`]；更新失败
//!    只是把这份缓存删掉（等价于"标 dirty"），下一次检索会从 PG 重新加载 —— 这样
//!    不需要一张 dirty 表，也不存在"索引里有、库里没有"的持久不一致。
//! 3. **查询只认 active generation**。重建期间新旧 generation 的向量行共存，
//!    [`ensure_loaded`] 按 `vector_collection_state.active_generation` 过滤；
//!    重建收尾翻转 active 之后调用 [`invalidate`] 让下一次查询重载。
//!
//! 常驻数量上限见 [`MAX_LOADED_COLLECTIONS`]：v1 先按"最多这么多集合"做 LRU，
//! 内存预算 + f16/i8 降精度属于 B-27 的完整版（见 §18）。

pub(crate) mod index;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::ai::embedding;
use crate::man::settings;
use crate::repo;
use crate::result::{Error, Result};

use index::{BruteForce, IndexBackend, ItemId, ScoredId, VectorIndex, VectorItem};

pub(crate) use index::is_hit;

/// 同时常驻的 `(robot_id, collection)` 上限。超了就淘汰最久没用过的那份。
///
/// 10 万条 × 1024 维 ≈ 400 MB，所以不能无限常驻；但也不能每次查询都重载 ——
/// 一个机器人的三类集合（phrase / qa / doc）加上多机器人，8 份是保守的起点。
pub(crate) const MAX_LOADED_COLLECTIONS: usize = 8;

type Key = (String, String);

struct Entry {
    last_used: u64,
    index: IndexBackend,
}

static INDEXES: LazyLock<Mutex<HashMap<Key, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::with_capacity(MAX_LOADED_COLLECTIONS)));
static CLOCK: AtomicU64 = AtomicU64::new(0);

fn tick() -> u64 {
    CLOCK.fetch_add(1, Ordering::Relaxed)
}

fn key(robot_id: &str, collection: &str) -> Key {
    (String::from(robot_id), String::from(collection))
}

fn lock() -> Result<std::sync::MutexGuard<'static, HashMap<Key, Entry>>> {
    INDEXES
        .lock()
        .map_err(|e| Error::WithMessage(format!("Vector index lock poisoned: {e}")))
}

fn is_loaded(robot_id: &str, collection: &str) -> bool {
    INDEXES
        .lock()
        .map(|m| m.contains_key(&key(robot_id, collection)))
        .unwrap_or(false)
}

/// 淘汰最久没用的那份，给新集合腾位置。
fn evict_lru(map: &mut HashMap<Key, Entry>) {
    if let Some(k) = map
        .iter()
        .min_by_key(|(_, e)| e.last_used)
        .map(|(k, _)| k.clone())
    {
        map.remove(&k);
    }
}

/// 从 PG 载入一份索引（已经载入就什么都不做）。
///
/// 读的是 `active_generation` 的向量行；没有 state 行（从没写过向量）时按 0 代读，
/// 自然得到空索引。
pub(crate) async fn ensure_loaded(robot_id: &str, collection: &str) -> Result<()> {
    if is_loaded(robot_id, collection) {
        return Ok(());
    }
    let rows = repo::vector::load_active(repo::pool(), robot_id, collection).await?;
    let rows: Vec<(ItemId, Vec<f32>)> = rows
        .into_iter()
        .map(|(id, vec)| (id as ItemId, vec))
        .collect();
    let backend = IndexBackend::BruteForce(BruteForce::from_items(rows)?);
    if let Ok(s) = backend.stats() {
        log::debug!(
            "Loaded vector index {robot_id}/{collection}: {} items × {} dims",
            s.len,
            s.dimension
        );
    }
    let mut map = lock()?;
    if !map.contains_key(&key(robot_id, collection)) {
        if map.len() >= MAX_LOADED_COLLECTIONS {
            evict_lru(&mut map);
        }
        map.insert(
            key(robot_id, collection),
            Entry {
                last_used: tick(),
                index: backend,
            },
        );
    }
    Ok(())
}

/// 检索：先保证载入，再在索引里取 top-`limit`。返回的 `score` 是相似度（D41）。
pub(crate) async fn search(
    robot_id: &str,
    collection: &str,
    query: &[f32],
    limit: usize,
) -> Result<Vec<ScoredId>> {
    ensure_loaded(robot_id, collection).await?;
    let mut map = lock()?;
    let Some(entry) = map.get_mut(&key(robot_id, collection)) else {
        // 并发 invalidate 抢在前面：当成空索引，不报错。
        return Ok(Vec::new());
    };
    entry.last_used = tick();
    let r = entry.index.search(query, limit);
    if r.is_err() {
        // 维度不一致等：缓存不可信，丢掉，让下一次查询从 PG 重载。
        map.remove(&key(robot_id, collection));
    }
    r
}

/// 写后同步：把刚提交的向量塞进已载入的索引；没载入就什么都不做（下次从 PG 读）。
pub(crate) fn upsert(robot_id: &str, collection: &str, rows: &[(ItemId, Vec<f32>)]) {
    if rows.is_empty() {
        return;
    }
    let Ok(mut map) = INDEXES.lock() else {
        return;
    };
    let k = key(robot_id, collection);
    let Some(entry) = map.get_mut(&k) else {
        return;
    };
    entry.last_used = tick();
    let items: Vec<VectorItem> = rows
        .iter()
        .map(|(id, vec)| VectorItem {
            id: *id,
            vec: vec.clone(),
        })
        .collect();
    if let Err(e) = entry.index.upsert(&items) {
        log::warn!("Updating vector index of {robot_id}/{collection} failed: {e}; dropping cache");
        map.remove(&k);
    }
}

/// 删除后同步。
pub(crate) fn delete(robot_id: &str, collection: &str, ids: &[ItemId]) {
    if ids.is_empty() {
        return;
    }
    let Ok(mut map) = INDEXES.lock() else {
        return;
    };
    let k = key(robot_id, collection);
    let Some(entry) = map.get_mut(&k) else {
        return;
    };
    entry.last_used = tick();
    if let Err(e) = entry.index.delete(ids) {
        log::warn!(
            "Deleting from vector index of {robot_id}/{collection} failed: {e}; dropping cache"
        );
        map.remove(&k);
    }
}

/// 作废一份缓存（换 generation、批量替换实体之后用）。
pub(crate) fn invalidate(robot_id: &str, collection: &str) {
    if let Ok(mut map) = INDEXES.lock() {
        map.remove(&key(robot_id, collection));
    }
}

/// 清空一份已载入的缓存（机器人 purge 用：库里行已被 FK 级联删掉，缓存也要变空）。
///
/// 与 [`invalidate`] 的区别：保留"已载入"状态并清成空索引，下次检索不必再从 PG 读
/// 一遍（读也只会读到空表）。
pub(crate) fn clear(robot_id: &str, collection: &str) {
    if let Ok(mut map) = INDEXES.lock()
        && let Some(entry) = map.get_mut(&key(robot_id, collection))
        && let Err(e) = entry.index.clear()
    {
        log::warn!("Clearing vector index of {robot_id}/{collection} failed: {e}");
        map.remove(&key(robot_id, collection));
    }
}

/// 当前配置会把向量算成什么模型（B-23 的 `embedding_models` 行）。
///
/// `dimension` 是**这次实际得到的**维度：设置里的 `dimensions` 只是用户期望，
/// 真实维度只有推理之后才知道，而 `embedding_models.fingerprint` 必须描述事实。
/// 因此这里的指纹和设置页用的 `current_embedding_identity()`（按配置拼、没设维度
/// 时用 0 占位）**不是**同一个东西：前者说"库里这份向量是什么"，后者说"用户想要
/// 什么样的向量"。
pub(crate) async fn current_model(
    robot_id: &str,
    dimension: usize,
) -> Result<repo::vector::ModelRef> {
    let Some(s) = settings::get_settings(robot_id).await? else {
        return Err(Error::WithMessage(format!(
            "Can not find settings of {robot_id}"
        )));
    };
    let p = &s.sentence_embedding_provider;
    let model = settings::embedding_model_name(&p.provider);
    let fingerprint = settings::embedding_identity(&p.provider, &model, dimension);
    let (provider, normalized) = match p.provider {
        // 本地 BERT 路径在 `hugging_face()` 里显式做了 L2 归一化。
        embedding::SentenceEmbeddingProvider::HuggingFace(_) => ("huggingface", true),
        embedding::SentenceEmbeddingProvider::OpenAICompatible(_) => ("openai-compatible", false),
    };
    Ok(repo::vector::ModelRef {
        provider: String::from(provider),
        model,
        dimension: dimension as i32,
        normalized,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 编码/解码是 `vector_items.vec` 的唯一格式约定，钉在这里。
    #[test]
    fn blob_roundtrip_is_little_endian_f32() {
        let v = vec![0.5f32, -1.5, 2.0];
        let blob = index::encode(&v);
        assert_eq!(blob.len(), 12);
        assert_eq!(index::decode(&blob).unwrap(), v);
        // 小端：0.5f32 = 0x3F000000 → 字节 00 00 00 3f
        assert_eq!(&blob[..4], &[0x00, 0x00, 0x00, 0x3f]);
    }

    /// LRU 的硬性质：超过上限必须淘汰，且淘汰的是最久没用的那份。
    #[test]
    fn lru_evicts_the_least_recently_used_collection() {
        let mut map: HashMap<Key, Entry> = HashMap::new();
        for (i, c) in ["a", "b", "c"].iter().enumerate() {
            map.insert(
                key("r", c),
                Entry {
                    last_used: i as u64,
                    index: IndexBackend::BruteForce(BruteForce::new(0)),
                },
            );
        }
        evict_lru(&mut map);
        assert!(!map.contains_key(&key("r", "a")), "最久没用的 a 应该被淘汰");
        assert!(map.contains_key(&key("r", "b")));
        assert!(map.contains_key(&key("r", "c")));
    }
}
