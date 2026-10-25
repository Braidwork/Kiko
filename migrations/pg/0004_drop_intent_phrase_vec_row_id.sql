-- 0004_drop_intent_phrase_vec_row_id.sql —— S4（B-30）
--
-- `intent_phrases.vec_row_id` 是 S3.3 的过渡列：短语载荷已经在 PG，向量还在 turso，
-- 用一个 BIGINT 行号指过去。S4 把向量搬进 `vector_items` 之后，向量身份统一成
-- `vector_items.id`（D7），这个既不是 API 字段也不是业务字段的列就没有意义了，
-- 而且它会让"实体与向量同事务"多一条要维护的引用。
--
-- migration 只加不改（0002 已应用过，改它会破坏 checksum），所以删除放到新文件里。

DROP INDEX IF EXISTS intent_phrases_vec_row_id;

ALTER TABLE intent_phrases DROP COLUMN IF EXISTS vec_row_id;
