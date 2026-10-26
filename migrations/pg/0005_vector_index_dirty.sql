-- 0005_vector_index_dirty.sql —— S4（B-28）
--
-- 进程内向量索引是缓存（D4）。更新索引失败、或后台对账发现"内存条目数 != PG 行数"时，
-- 把这一行标成 dirty；`vector::ensure_loaded` 成功从 PG 重建后清掉。
--
-- 只做标记，不做全量重建：dirty 的集合下一次检索就会重载；没有 jobs 表（D47）。

ALTER TABLE vector_collection_state
    ADD COLUMN IF NOT EXISTS dirty BOOLEAN NOT NULL DEFAULT FALSE;
