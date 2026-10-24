# AGENTS.md（Kiko / 项目 B）

本文件给 AI 编码助手用。目标：改得对、不误触发昂贵构建、不破坏已定决策。

---

## 1. 项目概览

- 原始项目：`D:\work\dialogflow-rs`，称之为：`A`，目前这个新项目：`Kiko`，称之为：`B`
- **Kiko（项目 B）**：`dialogflowai` 的企业私有化部署版本。技术上是 A 的 fork，
  存储层从 redb/turso 换成 **PostgreSQL**。
- **单二进制**：Rust 后端 + Vue3 前端，前端产物编进二进制，最终只有 `kiko(.exe)`
  一个可执行文件；**外部 PostgreSQL 由客户提供**，本地 `data/` 只放模型、上传原文件、配置文件。
- 技术栈：Rust edition 2024（**MSRV 1.94**）、axum 0.8、tokio、candle、**sqlx 0.9（PG）**、
  reqwest（rustls）；前端 Vue 3 + Vite + Element Plus + UnoCSS。
- 目录：`src/`（`web/` `repo/` `flow/` `intent/` `kb/` `ai/` `man/` `variable/` `external/`
  `robot/` `config/`）、`migrations/`、`frontend/`、`sdk/`、`doc/`、`data/`（gitignore）。
- 三个阶段的关系：**A** = 现有 `dialogflowai`（redb/turso，只读参考）；
  **B** = 本仓库；**C** = 将来的信创/政企版，基于 B 演进。

---

## 2. 文档地图（开工前必读）

**唯一权威文档：`doc/kiko-plan.md`**

| 文档 | 作用 | 什么时候读 |
| --- | --- | --- |
| `doc/kiko-plan.md` | 决策 D1–D41、完整 schema（§5）、机制（§6）、行动项 B-xx（§9）、执行顺序与阶段门（§15）、A 带过来的坑（§16） | **每次开工必读 §15.1 进度表 + §15.2**；写代码前查 §5 与对应 B-xx |
| `doc/streaming.md` | `flow/answer` 流式协议契约 | 改动流式协议时必读 |

规则：

1. 文档与代码冲突时，先确认是文档过期还是代码写错，**不要自己选一个**。
2. **不要修改 `doc/kiko-plan.md` 里已定的决策（D 编号）**；要改先问。
3. 任务用「阶段 + B-xx」定位，例如「S3.3 / B-14」。一次只推进一个阶段（S0…S5）。
4. 改 schema 必须同时改 `migrations/` 和 `doc/kiko-plan.md §5`；
   **migration 文件是事实，文档是意图**，两者不一致视为 bug。
5. 每完成一个阶段/模块，更新 `doc/kiko-plan.md §15.1` 的进度表。

---

## 3. 命令约定（直接敲命令，不要用 .bat）

A 的那些 `r.bat` / `t.bat` / `z.bat` / `frontend/*.bat` **在 Kiko 里不要用**
（脚本里有 `cls`、全局 `RUST_LOG`、通配删除/覆盖拷贝等副作用）。

| 目的 | 用这个 | 不要用 |
| --- | --- | --- |
| 编译检查（快） | `cargo check` | `cargo build`（除非确实要跑二进制） |
| 静态检查 / 格式 | `cargo clippy`、`cargo fmt` | 手改格式 |
| 单元测试 | `cargo test` | — |
| 集成测试 | `KIKO_TEST_DATABASE_URL=postgres://… cargo test` | 依赖本机默认库 |
| 起服务验证 | 自己构建的 `target/.../kiko.exe`，用完 `taskkill` | 长期占端口 |
| 前端开发 | `pnpm dev`（在 `frontend/`） | — |
| 前端产物进二进制 | **默认不做**，见第 4 节 | `frontend/b.bat` |

- 不要为了验证跑全量 `cargo build --release`；`cargo check` / `cargo test` / 单测过滤够用。
- 不要主动联网调真实大模型 / 外部 API 验证；需要时起本地假服务（临时脚本放系统临时目录，用完删）。
- 不要长期留着后台服务进程。

---

## 4. 前端改动：不要动 `src/resources/assets/`，不要跑 `b.bat`

链路：`frontend/b.bat` 会 `pnpm build` → 覆盖 `sdk/javascript/DialogFlowAiSDK.min.js` →
清空并全量拷贝 `src/resources/assets/`（带内容哈希的文件名）→ `build.rs` 重新生成
`src/web/asset.{rs,txt}` → 触发整棵 Rust 重新编译。

因此：

- 只改 `frontend/src/**` 时，**只改源码**，不 `pnpm build`、不拷 `dist`、不手改生成物。
- `dist/`、`src/resources/assets/`、`src/web/asset.{rs,txt}` 都是生成物：不提交、不手改。
- 只有用户明确要求出包时，才执行前端构建，并如实报告 `git status` 里的增删。
- 不要用整目录删除来「清理」。

---

## 5. 硬约束

1. **SQL 只出现在 `src/repo/`**（以及 `migrations/`）。业务模块、handler、`flow/` 里不许出现裸 SQL。
2. **业务模块不直接持有 `sqlx::Pool`**，只依赖 repo 的领域接口。
3. **redb 已移除**（B-20，`cargo tree` 无 redb）；`src/db/`、`db_executor!` 不再存在。
   turso 只剩知识库/向量（S4 搬完前保留），**不允许在 turso 上加新表/新字段**，
   新功能一律落在 PG。
4. **一个模块的数据只在一个库里**，禁止双写。跨库写（purge）必须两段 + 幂等 + 可重试。
5. **不做数据迁移**：不读 A 的 `data/*.dat`，不写兼容层。
6. 不引入 ORM、不引入策略引擎；手写 SQL + repo 层。
7. 不引入新的嵌入式数据库引擎；redb 已归零，turso 只减不增（S4 归零）。
8. **默认不联网**：不新增任何向公网发请求的代码（版本检查默认关闭，D38）。
9. 敏感字段一律走 `secrets` 表 + 引用串（`enc:` / `env:` / `file:`），
   **不落日志、不回显 `***`**。
10. 未经确认不 `git commit` / `git push`，不改 `.github/workflows/` 的触发条件。

---

## 6. 代码风格

- 以 `cargo fmt` + `rustfmt.toml` 为准；提交前 `cargo fmt --check`、`cargo clippy` 干净。
- 错误处理沿用 `anyhow` + 现有 `Result` 约定；不要引入新的错误处理框架。
- 线上 JSON **保持 camelCase**（Rust 字段 snake_case + `#[serde(rename_all = "camelCase")]`）。
- HTTP 信封保持 `{status, data, err}`；标准 HTTP 状态码可以叠加，但**不许改变现有形状**。
- 公开主键是纯 scru128 字符串（27 字符），**没有** `r` 前缀或数字 ID；内部行号不许泄漏到 API。
- 中文注释/文档与英文混用是现状，跟随所改文件。
- Cargo.toml 里被注释掉的历史依赖是既有风格，不要顺手大清理。

---

## 7. 测试

- 新逻辑就近加单测（`#[test]` / `#[tokio::test]`），不要只靠手动 curl。
- **存储路径必须有集成测试**：每个测试自建 schema（`CREATE SCHEMA tmp_<uuid>` + `search_path`）
  并在结束时 drop；不允许污染共享库。
- 改动涉及数据时用临时 schema/临时目录，跑完清理，`git status` 要保持干净。
- 已知既有失败测试（若有）不因为你的改动而新增；`html_to_plain` 这类要么修掉要么明确标注。
- Windows 上做浏览器行为验证用无头 Edge/Chrome，探针脚本放临时目录并删除。

---

## 8. 与 A 的关系

- A 的仓库**只读**：可以去看 A 的实现作为语义参考（尤其是 `flow/`、`intent/`、`kb/` 的业务行为），
  但**不要修改 A**，也不要把 A 的存储代码直接搬进 B。
- A 带过来的已知坑（`deser_node` 的 `unwrap`、`phrase::search` 里的活调试块、
  读-改-写丢更新、两套距离语义）见 `doc/kiko-plan.md §16`，迁移时必须处理。
- 行为要等价：迁移一个模块前后，同一操作的 HTTP 输出、排序、错误码应一致；
  有意的差异（如分数方向）按 D41 统一并留对照测试。

---

## 9. 交付习惯

报告要包含：

- 改了哪些文件、跑了哪些命令、**实际输出结论**（通过/失败）；
- **没做什么**（例如「未执行前端构建、未重新生成 resources/assets、未跑 100 万条压测」）；
- 该阶段的验收项逐条对照（`doc/kiko-plan.md §15.4`）；
- 若改动了 schema 或决策相关的实现，注明是否已同步 `doc/kiko-plan.md`。

不要声称验证过却没跑；跑不了就说明原因。用中文提问就用中文回答。

---

## 10. 环境信息
- `PostgreSQL`登录用户：`postgres`，密码：`111111`
