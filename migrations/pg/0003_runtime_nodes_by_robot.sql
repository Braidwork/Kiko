-- 0003_runtime_nodes_by_robot.sql —— S3.5 运行时节点
--
-- 原 schema 的 runtime_nodes 只有 `main_flow_id`，并外键指向 `mainflows(id)`。
-- 实际有两个问题：
--
--   1. **demo 流程没有 mainflows 行**：`flow/demo/*.txt` 是内置模板，`/mainflow/release`
--      会为 `demo-collect` 等 id 生成运行时节点，但没有对应的主流程行。保留外键会让
--      demo 发布直接失败。
--   2. 运行时节点是**派生数据**（release 时从 canvas 重新生成），不是主流程的子实体；
--      它的生命周期应该跟着机器人走，而不是跟着某一行主流程。
--
-- 因此改成 (robot_id, main_flow_id, node_id) 三列主键：
--   * robot_id 外键 → robots ON DELETE CASCADE（删机器人自动清节点）；
--   * main_flow_id 只是命名空间（真实主流程 id 或 demo-* 模板 id），不再外键；
--     删主流程时由 `repo::mainflow::delete` 在同一事务里显式清掉它的节点。
--
-- 现有的 runtime_nodes 行都是派生数据，且本迁移之前该表从未被写入（模块仍在 redb），
-- 直接清空即可，不涉及数据迁移。

DELETE FROM runtime_nodes;

ALTER TABLE runtime_nodes DROP CONSTRAINT IF EXISTS runtime_nodes_main_flow_id_fkey;
ALTER TABLE runtime_nodes DROP CONSTRAINT IF EXISTS runtime_nodes_pkey;

ALTER TABLE runtime_nodes
    ADD COLUMN robot_id VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE;

ALTER TABLE runtime_nodes
    ADD PRIMARY KEY (robot_id, main_flow_id, node_id);
