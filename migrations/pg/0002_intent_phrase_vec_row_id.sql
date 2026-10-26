-- 0002_intent_phrase_vec_row_id.sql —— S3.3（B-14）的过渡列
--
-- 意图短语的**载荷**已经落在 PG（`intent_phrases`），但 `phrase.dat`（turso）里
-- 还留着对应的向量行。删除/重算一条短语时要能定位到那一行，所以在 PG 侧记下
-- turso 的行号。
--
-- 这是过渡列：S4 把向量搬进 PG 的 `vector_items` 之后，向量行身份改用
-- `vector_items.id`（D7），本列随 B-30 一起删除。在此之前**不要**把它当成
-- 业务主键或暴露给 API —— 对外只有 `intent_phrases.id`（scru128 字符串）。
--
-- 允许 NULL：历史行/迁移前没有向量行的短语（它们会被下一次重建补上）。

ALTER TABLE intent_phrases ADD COLUMN vec_row_id BIGINT;
