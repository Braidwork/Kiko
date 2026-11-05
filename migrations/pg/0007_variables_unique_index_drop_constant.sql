-- 0007_variables_unique_index_drop_constant.sql
--
-- 1. 为 var_unified_name 建显式的命名唯一索引（0006 的 PRIMARY KEY 已经隐含一个
--    唯一 B-tree，这里按运维/查询习惯把名字显式化；重复索引对该小表无性能影响）。
-- 2. 去掉 var_constant_value：变量不再保存“常量值”这一来源。

-- CREATE UNIQUE INDEX IF NOT EXISTS variables_var_unified_name_uniq
--     ON variables (var_unified_name);

ALTER TABLE variables DROP COLUMN IF EXISTS var_constant_value;
