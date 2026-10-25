//! 进程内向量索引的**注册表**：按 `(robot_id, collection)` 懒加载、按内存预算淘汰、
//! 写后同步、换 generation 后作废（D35 / D47 / §6.3）。
//!
//! 四条硬规矩：
//!
//! 1. **PG 是事实，索引是缓存**（D4）。索引永远从 `vector_items` 重建，不落磁盘；
//!    进程重启、加载失败、维度不一致，都只需要"丢掉缓存、下次从 PG 重读"。
//! 2. **先写 PG，后更新索引**。写完提交成功才调 [`upsert`] / [`delete`]；更新失败
//!    会把缓存删掉并把 `vector_collection_state.dirty` 标上（B-28），下一次检索从
//!    PG 重新加载，加载成功后再把 dirty 清掉。
//! 3. **查询只认 active generation**。重建期间新旧 generation 的向量行共存，
//!    [`ensure_loaded`] 按 `vector_collection_state.active_generation` 过滤；
//!    重建收尾翻转 active 之后调用 [`invalidate`] 让下一次查询重载。
//! 4. **内存有预算**（B-27）。加载前先按 `(行数, 维度)` 估算占用；超过
//!    `KIKO_VECTOR_INDEX_BUDGET_MB`（默认 512）就先降成 f16，再把最久没用的集合
//!    淘汰掉。还放不下就记一条 warn 照常加载 —— 更大规模要等 ANN（C 阶段）。
//!
//! 后台对账（行数 vs PG）在 [`reconcile`] 里，由 `start_app` 启动。

pub(crate) mod index;
pub(crate) mod reconcile;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::ai::embedding;
use crate::man::settings;
use crate::repo;
use crate::result::{Error, Result};

use index::{
    BruteForce, IndexBackend, ItemId, Precision, ScoredId, VectorIndex, VectorItem,
    choose_precision,
};

pub(crate) use index::is_hit;

/// 常驻集合数硬上限。字节预算是主约束，这个只是防御（防止几千个小集合）。
pub(crate) const MAX_LOADED_COLLECTIONS: usize = 8;

/// 默认内存预算：512 MB。10 万条 × 1024 维 f32 ≈ 400 MB，f16 ≈ 204 MB。
const DEFAULT_BUDGET_MB: usize = 512;
const BUDGET_ENV: &str = "KIKO_VECTOR_INDEX_BUDGET_MB";

/// 索引内存预算（字节）。`KIKO_VECTOR_INDEX_BUDGET_MB` 覆盖，`0` / 非法值回落默认。
pub(crate) fn budget_bytes() -> usize {
    static BUDGET: LazyLock<usize> = LazyLock::new(|| {
        std::env::var(BUDGET_ENV)
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|m| *m > 0)
            .unwrap_or(DEFAULT_BUDGET_MB)
            .saturating_mul(1024 * 1024)
    });
    *BUDGET
}

type Key = (String, String);

struct Entry {
    last_used: u64,
    /// 估算的常驻字节数（`BruteForce::bytes()`）。
    bytes: usize,
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

/// 最久没用过的集合（排除正要插入的 `keep`）。
fn lru_victim(map: &HashMap<Key, Entry>, keep: &Key) -> Option<Key> {
    map.iter()
        .filter(|(k, _)| *k != keep)
        .min_by_key(|(_, e)| e.last_used)
        .map(|(k, _)| k.clone())
}

/// 为 `needed` 字节腾地方：先按字节预算淘汰，再按集合数上限淘汰。
///
/// 返回淘汰掉的集合数（测试用）。
fn evict_until(
    map: &mut HashMap<Key, Entry>,
    keep: &Key,
    needed: usize,
    budget: usize,
    max_count: usize,
) -> usize {
    let mut evicted = 0;
    let mut used: usize = map
        .iter()
        .filter(|(k, _)| *k != keep)
        .map(|(_, e)| e.bytes)
        .sum();
    while used.saturating_add(needed) > budget {
        let Some(victim) = lru_victim(map, keep) else {
            break;
        };
        used = used.saturating_sub(map.get(&victim).map(|e| e.bytes).unwrap_or(0));
        map.remove(&victim);
        evicted += 1;
    }
    while map.keys().filter(|k| *k != keep).count() >= max_count {
        let Some(victim) = lru_victim(map, keep) else {
            break;
        };
        map.remove(&victim);
        evicted += 1;
    }
    evicted
}

/// 从 PG 载入一份索引（已经载入就什么都不做）。
///
/// 读的是 `active_generation` 的向量行；没有 state 行（从没写过向量）时按 0 代读，
/// 自然得到空索引。行是**流式**读的：`vector_items.vec` 只按行解码，不会先把整张
/// 表拉成 `Vec<[f32]>`（否则降精度之前就已经把内存吃掉了）。
pub(crate) async fn ensure_loaded(robot_id: &str, collection: &str) -> Result<()> {
    if is_loaded(robot_id, collection) {
        return Ok(());
    }
    let pool = repo::pool();
    let state = repo::vector::state(pool, robot_id, collection).await?;
    let (generation, dirty) = match &state {
        Some(s) => (s.active_generation, s.dirty),
        None => (0, false),
    };
    let budget = budget_bytes();
    // 加载期间可能有并发写落库：读到的行数如果和构建后的条目数对不上，就再读一次。
    // 两次都对不上（写入持续不停）也照常缓存，但标 dirty，交给后台对账兜底。
    let mut consistent = false;
    let mut loaded: Option<(IndexBackend, index::IndexStats)> = None;
    for attempt in 0..2 {
        let one = load_generation(robot_id, collection, generation).await?;
        let now = repo::vector::count_generation(pool, robot_id, collection, generation).await?;
        consistent = now as usize == one.1.len;
        loaded = Some(one);
        if consistent {
            break;
        }
        log::debug!(
            "Vector index {robot_id}/{collection} changed while loading (attempt {attempt}); retrying"
        );
    }
    let (backend, stats) = loaded.expect("循环至少执行一次");
    let bytes = stats.bytes;
    if stats.precision == Precision::F16 {
        log::info!(
            "Vector index {robot_id}/{collection}: {} rows × {} dims exceeds the {budget} B budget, using f16 ({bytes} B)",
            stats.len,
            stats.dimension
        );
    }
    {
        let mut map = lock()?;
        let k = key(robot_id, collection);
        if !map.contains_key(&k) {
            evict_until(&mut map, &k, bytes, budget, MAX_LOADED_COLLECTIONS);
            map.insert(
                k,
                Entry {
                    last_used: tick(),
                    bytes,
                    index: backend,
                },
            );
        }
    }
    log::debug!(
        "Loaded vector index {robot_id}/{collection}: {} items × {} dims, {bytes} bytes",
        stats.len,
        stats.dimension
    );
    if bytes > budget {
        log::warn!(
            "Vector index {robot_id}/{collection} needs {bytes} B, over the {budget} B budget even at f16; \
             consider shrinking the collection or an ANN backend (C phase)"
        );
    }
    if !consistent {
        // 缓存是从一份正在变化的快照建出来的：先标脏，下一次对账/重载会纠正。
        mark_dirty(robot_id, collection).await;
    } else if dirty {
        // 加载成功 = 缓存与 PG 一致；把"可能不一致"的标记清掉（B-28）。
        repo::vector::set_dirty(pool, robot_id, collection, false).await?;
    }
    Ok(())
}

/// 一个 generation 的完整索引（不碰缓存表）。
async fn load_generation(
    robot_id: &str,
    collection: &str,
    generation: i64,
) -> Result<(IndexBackend, index::IndexStats)> {
    let pool = repo::pool();
    let rows =
        repo::vector::count_generation(pool, robot_id, collection, generation).await? as usize;
    let dim = repo::vector::generation_dim(pool, robot_id, collection, generation).await?;
    let precision = choose_precision(rows, dim, budget_bytes());
    let mut brute = BruteForce::new(dim, precision);
    repo::vector::for_each_generation_row(pool, robot_id, collection, generation, |id, blob| {
        let vec = index::decode(blob)?;
        brute.insert(id as ItemId, &vec)
    })
    .await?;
    let stats = brute.stats()?;
    Ok((IndexBackend::BruteForce(brute), stats))
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
///
/// 更新失败 → 丢掉缓存 + 标 dirty（B-28）。`settle` 之后由下一次检索重载并清标记。
pub(crate) async fn upsert(robot_id: &str, collection: &str, rows: &[(ItemId, Vec<f32>)]) {
    if rows.is_empty() {
        return;
    }
    let mut failed = false;
    {
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
        match entry.index.upsert(&items) {
            Ok(()) => {
                if let Ok(s) = entry.index.stats() {
                    entry.bytes = s.bytes;
                }
            }
            Err(e) => {
                log::warn!(
                    "Updating vector index of {robot_id}/{collection} failed: {e}; dropping cache and marking dirty"
                );
                map.remove(&k);
                failed = true;
            }
        }
    }
    if failed {
        mark_dirty(robot_id, collection).await;
    }
}

/// 删除后同步。删除本身不会让内存变大，但失败同样要标 dirty。
pub(crate) async fn delete(robot_id: &str, collection: &str, ids: &[ItemId]) {
    if ids.is_empty() {
        return;
    }
    let mut failed = false;
    {
        let Ok(mut map) = INDEXES.lock() else {
            return;
        };
        let k = key(robot_id, collection);
        let Some(entry) = map.get_mut(&k) else {
            return;
        };
        entry.last_used = tick();
        match entry.index.delete(ids) {
            Ok(()) => {
                if let Ok(s) = entry.index.stats() {
                    entry.bytes = s.bytes;
                }
            }
            Err(e) => {
                log::warn!(
                    "Deleting from vector index of {robot_id}/{collection} failed: {e}; dropping cache and marking dirty"
                );
                map.remove(&k);
                failed = true;
            }
        }
    }
    if failed {
        mark_dirty(robot_id, collection).await;
    }
}

async fn mark_dirty(robot_id: &str, collection: &str) {
    if let Err(e) = repo::vector::set_dirty(repo::pool(), robot_id, collection, true).await {
        log::warn!("Marking vector index of {robot_id}/{collection} dirty failed: {e}");
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

/// 当前载入的集合（对账用）。
pub(crate) fn loaded_keys() -> Vec<(String, String)> {
    INDEXES
        .lock()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// 某个集合载入的条目数；没载入返回 `None`。
pub(crate) fn loaded_count(robot_id: &str, collection: &str) -> Option<usize> {
    let map = INDEXES.lock().ok()?;
    map.get(&key(robot_id, collection))
        .and_then(|e| e.index.stats().ok())
        .map(|s| s.len)
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

    fn entry(last_used: u64, bytes: usize) -> Entry {
        Entry {
            last_used,
            bytes,
            index: IndexBackend::BruteForce(BruteForce::new(0, Precision::F32)),
        }
    }

    /// LRU 的硬性质：超预算必须淘汰，且淘汰的是最久没用的那份。
    #[test]
    fn evicts_the_least_recently_used_collection_to_fit_the_budget() {
        let mut map: HashMap<Key, Entry> = HashMap::new();
        for (i, c) in ["a", "b", "c"].iter().enumerate() {
            map.insert(key("r", c), entry(i as u64, 100));
        }
        // 需要 150 字节、预算 350：现有 300 → 450 > 350，淘汰一份最久没用的（a）
        // 之后 200 + 150 = 350 正好放下。
        let evicted = evict_until(&mut map, &key("r", "d"), 150, 350, 8);
        assert_eq!(evicted, 1);
        assert!(!map.contains_key(&key("r", "a")));
        assert!(map.contains_key(&key("r", "b")));
        assert!(map.contains_key(&key("r", "c")));
    }

    /// 预算永远优先淘汰最久没用的，直到放得下；放不下就全部淘汰（调用方会 warn）。
    #[test]
    fn eviction_continues_until_the_new_collection_fits() {
        let mut map: HashMap<Key, Entry> = HashMap::new();
        for (i, c) in ["a", "b", "c"].iter().enumerate() {
            map.insert(key("r", c), entry(i as u64, 100));
        }
        let evicted = evict_until(&mut map, &key("r", "new"), 900, 250, 8);
        assert_eq!(evicted, 3, "900 字节放不进 250 的预算，必须清空");
        assert!(map.is_empty());
    }

    /// 集合数上限：字节够也必须淘汰，否则小集合会无限积累。
    #[test]
    fn collection_count_cap_evicts_even_when_the_budget_is_loose() {
        let mut map: HashMap<Key, Entry> = HashMap::new();
        for (i, c) in ["a", "b", "c"].iter().enumerate() {
            map.insert(key("r", c), entry(i as u64, 1));
        }
        evict_until(&mut map, &key("r", "d"), 1, usize::MAX, 3);
        assert!(!map.contains_key(&key("r", "a")), "最久没用的 a 应先走");
        assert_eq!(map.len(), 2);
    }

    /// 正在插入的集合自己不能被当成淘汰对象。
    #[test]
    fn the_collection_being_loaded_is_never_evicted() {
        let mut map: HashMap<Key, Entry> = HashMap::new();
        map.insert(key("r", "x"), entry(0, 100));
        evict_until(&mut map, &key("r", "x"), 1000, 0, 8);
        assert!(map.contains_key(&key("r", "x")));
    }

    /// B-27：预算默认存在、可用环境变量覆盖（这里只断言非零，避免改进程环境）。
    #[test]
    fn budget_defaults_to_a_positive_number_of_bytes() {
        assert!(budget_bytes() >= 8 * 1024 * 1024);
    }
}
