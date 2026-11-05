-- 0006_variables_global.sql —— 变量改为跨机器人共享，并引入内部归一化名与三态缓存。
--
-- 决策：变量对所有机器人可见，表里不再有 robot_id；程序内部（流程表达式、
-- 会话缓存、SQL 查询）统一使用 var_unified_name（归一化后的 code），var_name
-- 只作为显示名。
--
-- val_cache_type 只有 0/1/2 三个值：0=不缓存，1=按请求缓存，2=按会话缓存。
-- PostgreSQL 没有 TINYINT，用 SMALLINT + CHECK 表达同样的取值范围。
--
-- 新项目，不涉及旧数据迁移 —— 直接重建表。

DROP TABLE IF EXISTS variables;

CREATE TABLE variables (
    var_unified_name               TEXT PRIMARY KEY,
    var_name                       TEXT NOT NULL,
    var_type                       TEXT NOT NULL,
    var_value_source               TEXT NOT NULL,
    var_constant_value             TEXT,
    var_associate_data             TEXT,
    obtain_value_expression_type   TEXT,
    obtain_value_expression        TEXT,
    connect_timeout_ms             BIGINT,
    read_timeout_ms                BIGINT,
    val_cache_type                 SMALLINT NOT NULL DEFAULT 2
                                   CHECK (val_cache_type IN (0, 1, 2, 3)),
    created_at                     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                     TIMESTAMPTZ NOT NULL DEFAULT now(),
    version                        BIGINT NOT NULL DEFAULT 0
);
