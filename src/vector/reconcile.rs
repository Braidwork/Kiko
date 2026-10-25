//! 后台对账 + dirty 标记（B-28）。
//!
//! 索引是缓存，PG 是事实。绝大多数不一致都能靠"写失败就丢缓存、下次从 PG 重载"
//! 自愈，但有两类漂移它看不见：
//!
//! 1. **进程外的写入**（手工 SQL、未来多实例）—— 缓存还在，行数却变了；
//! 2. **更新索引失败后没有下一次查询**（dirty 标记落库了，但没人去修）。
//!
//! 所以这里做两件事：
//!
//! - 每 `KIKO_VECTOR_RECONCILE_SECS`（默认 60，`0` 关闭）检查一次**已载入**的集合：
//!   `内存条目数 != PG 里 active generation 的行数` 就作废缓存并标 dirty。下一次
//!   检索会从 PG 重载，加载成功时把 dirty 清掉（见 `vector::ensure_loaded`）。
//! - 暴露 [`needs_rebuild`]，让判断本身可以被单测钉住。
//!
//! 这里**不**做全量重建：只对"已经载入且对不上"的集合动手，冷集合本来就没有缓存
//! 可对。也不建 jobs 表（D47）。

use std::time::Duration;

use super::{invalidate, loaded_count, loaded_keys};
use crate::repo;
use crate::result::Result;

/// 默认对账周期。
const DEFAULT_SECS: u64 = 60;
const ENV: &str = "KIKO_VECTOR_RECONCILE_SECS";

/// 对账周期；`KIKO_VECTOR_RECONCILE_SECS=0` 关闭（返回 `None`）。
pub(crate) fn interval() -> Option<Duration> {
    let secs = std::env::var(ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SECS);
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs))
    }
}

/// 启动后台对账任务（`start_app` 调一次）。
pub(crate) fn spawn() {
    let Some(interval) = interval() else {
        log::info!("Vector index reconciliation is disabled ({ENV}=0)");
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match reconcile_once().await {
                Ok(0) => (),
                Ok(n) => {
                    log::warn!("Vector index reconciliation invalidated {n} stale collection(s)")
                }
                Err(e) => log::warn!("Vector index reconciliation failed: {e}"),
            }
        }
    });
    log::info!("Vector index reconciliation runs every {interval:?}");
}

/// 内存条目数与 PG 的 active 行数不一致就重建。
pub(crate) fn needs_rebuild(loaded: usize, active: usize) -> bool {
    loaded != active
}

/// 跑一轮对账：返回被作废（并标 dirty）的集合数。
///
/// 只检查**已经载入**的集合；`active` 取的是 active generation 的行数，与加载时
/// 用的口径一致。
pub(crate) async fn reconcile_once() -> Result<usize> {
    let mut fixed = 0;
    for (robot_id, collection) in loaded_keys() {
        let Some(loaded) = loaded_count(&robot_id, &collection) else {
            continue;
        };
        let active =
            repo::vector::count_active(repo::pool(), &robot_id, &collection).await? as usize;
        if needs_rebuild(loaded, active) {
            log::warn!(
                "Vector index {robot_id}/{collection} has {loaded} items but PG has {active}; dropping cache"
            );
            invalidate(&robot_id, &collection);
            repo::vector::set_dirty(repo::pool(), &robot_id, &collection, true).await?;
            fixed += 1;
        }
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_count_mismatch_is_stale() {
        assert!(!needs_rebuild(0, 0));
        assert!(!needs_rebuild(10, 10));
        assert!(needs_rebuild(10, 11), "PG 多了一条 → 缓存过期");
        assert!(needs_rebuild(11, 10), "PG 少了一条 → 缓存过期");
    }

    /// 默认 60 秒；这里不改进程环境（测试并行），只断言解析结果非零。
    #[test]
    fn interval_defaults_to_sixty_seconds() {
        assert_eq!(
            interval().map(|d| d.as_secs()),
            Some(DEFAULT_SECS),
            "没设 KIKO_VECTOR_RECONCILE_SECS 时应该是 60 秒"
        );
    }
}
