简体中文 | [English](./README.md)（上游版本，Kiko 的差异以本页为准）

欢迎给本项目，或者给 [Github 上的项目](https://github.com/Braidwork/Kiko) ✨**Star**🎇!

# Kiko —— 可私有化部署的对话机器人平台

Kiko 是一个**可私有化部署的对话机器人平台**。用拖拽式流程编排对话，用意图/关键词/正则
理解用户，用知识库做检索问答，也可以接入本地模型或任何 `OpenAI` 兼容接口。它交付为一个
可执行文件，业务数据放在你自己的 `PostgreSQL` 里——适合"数据不能上公有云、又不想维护
一堆中间件"的场景。

它是 `dialogflow-rs`（Apache-2.0）的企业私有化分支。和上游最大的不同在**存储**：知识库、
意图、流程、会话与向量全部进客户自己的 `PostgreSQL`，`data/` 只放模型权重、上传的原文件、
配置和主密钥。换来的是可运维性：备份、恢复、监控、审计、容量规划都沿用客户现成的
PG 体系，而不是散落在 `data/` 里的二进制文件。

<img src="https://img.shields.io/badge/version-1.24.0-blue" /> <img src="https://img.shields.io/badge/MSRV-1.94-green" /> <img src="https://img.shields.io/badge/PostgreSQL-14%2B-336791" /> <img src="https://img.shields.io/badge/license-Apache--2.0-lightgrey" />

![All in ONE](./doc/assets/allinone.png)

![](./doc/assets/screenshots/flow-editor-zh.png)

## Kiko 和 dialogflow-rs 的差异

| 维度 | dialogflow-rs（上游） | **Kiko（本仓库）** |
| --- | --- | --- |
| 定位 | 开箱即用的单机版 | 企业私有化部署版，面向"客户提供数据库"的交付场景 |
| 业务数据 | 嵌入式 redb（后期 turso） | **外部 PostgreSQL**，固定 schema `kiko`，不占用 `public` |
| 知识库 / 向量 | 与元数据同一套嵌入式存储 | `documents` / `doc_chunks` / `qa` + `vector_items` 等全部进 PG |
| 安装依赖 | 0 依赖 | 需要 PostgreSQL（开发验证用 18，建议 14+） |
| schema 管理 | 无（数据文件随程序走） | `sqlx::migrate!` 内嵌 migration，启动自动迁移（`auto_migrate`） |
| 并发正确性 | 单写队列，读-改-写有丢更新窗口 | 事务 + 行锁 + **全部可编辑实体的乐观锁**（`version`），冲突显式报错 |
| 敏感配置 | 本地明文 | `secrets` 表 + `enc:`/`env:`/`file:` 引用串，AES-GCM 加密，主密钥可外置 |
| 换嵌入模型 | 改配置即生效（可能新旧向量混用） | **active model 分离**：查询/增量写跟随"已建索引的模型"，只有重建才切换 |
| 向量索引 | turso 内检索 | PG 为事实源 + 进程内暴力索引（内存预算、f16 降精度、dirty 标记、后台对账） |
| 重启与续跑 | 依赖 redb/turso 文件的一致性 | 运行态也在 PG；重建扫描幂等，可中断续跑（无 jobs 表） |
| 配置 | 环境变量/内置 | `data/kiko.toml` + `KIKO_*` 覆盖（优先级：环境变量 > 文件 > 默认） |
| API 契约 | 含内部行号 | 对外只用 scru128 主键，内部行号不外泄 |

## ✨ 亮点

* 🗄️ **数据库即事实源**：没有嵌入式库、没有 turso/redb 文件锁。备份 = PG 备份
  + `data/uploads/`（外加 `data/master.key`），恢复、主从、监控都交给客户已有的 PG 体系。
* 📦 **仍是单二进制**：Rust 后端 + Vue3 前端编进一个可执行文件；`data/` 只放模型权重、
  上传的原文件、`kiko.toml` 和主密钥，业务数据一行不落本地。
* 🔎 **知识库可审计**：DOCX 上传 → 分块（原文 `chunk_text` + 归一化 `chunk_text_norm`）
  → 向量，原文可查可回溯；关键词召回与向量召回用 RRF 融合。
* 🧠 **向量索引可运维**：`[vector]` 配置内存预算（默认 512 MB），超预算先降 f16、再按
  LRU 淘汰整个集合；后台按行数与 PG 对账，漂移就标 `dirty`；重建按 generation 幂等续跑。
* 🔁 **换模型不踩坑**：查询与增量写入使用 `active_model_id` 指向的模型，重建完成后才
  整体切换；不会出现"新查询向量 vs 旧库向量"的静默错误。
* 🔐 **敏感配置不落明文**：API key / 密码以引用串存储，主密钥可用 `KIKO_MASTER_KEY`
  外置；不写日志、不回显。
* 🧩 **并发安全**：文档、问答、画布、主流程、机器人、变量、外部 HTTP 的编辑都带
  `version` 校验；缺版本直接拒绝，旧版本返回冲突，不再静默覆盖别人的修改。
* ⚙️ **配置透明**：`data/kiko.toml` 模板带完整注释，所有 `KIKO_*` 覆盖项都有一一对应。
* 🧪 **可测试**：PG 集成测试用 `tmp_<uuid>` schema，跑完即删，不污染共享库。
* 🧬 **继承上游**：流程引擎、9 类节点、意图/关键词/正则/短语、流式回答、OpenAI 兼容
  接口与本地 `HuggingFace` 模型全部保留，前端就是同一套 Vue3 界面。

## 关键特性（继承自上游）

* 🐱‍🏍 **AI 驱动**：本地 `Huggingface` 模型（Llama、Phi-3、Gemma、Multilingual E5、
  MiniLM L6v2、NomicEmbedTextV1_5 等），以及任何 OpenAI-compatible 接口（OpenAI、
  DeepSeek、智谱 GLM、通义千问、Kimi、硅基流动、Groq、OpenRouter、Ollama、vLLM、
  LM Studio 或自建网关），可用于流程聊天、答案生成与意图识别。
* 😀 **可视化编排**：拖拽节点即可搭机器人，支持条件分支、信息收集、跳转、外部 HTTP、
  邮件、知识库答案等。
* 🚀 **Rust + Vue**：单进程、多线程运行时，无 Node/Python 运行时依赖。
* 🔐 **数据不出域**：业务数据在客户 PG，模型与上传文件在客户 `data/`；只有显式配置
  外部模型 API 时才会出网。

## 功能节点

| 节点 | 名称 |
| --- | --- |
| ![DialogNode](./doc/assets/screenshots/dialogNode.png) | 对话答案节点 |
| ![LLM chat node](./doc/assets/screenshots/llmChatNode.png) | 大模型聊天节点 |
| ![](./doc/assets/screenshots/knowledgeBaseAnswerNode.png) | 知识库答案节点 |
| ![](./doc/assets/screenshots/conditionNode.png) | 条件节点 |
| ![](./doc/assets/screenshots/gotoNode.png) | 跳转节点 |
| ![](./doc/assets/screenshots/collectNode.png) | 信息收集节点 |
| ![](./doc/assets/screenshots/externalApiNode.png) | 请求外部接口节点 |
| ![](./doc/assets/screenshots/sendEmailNode.png) | 邮件发送节点 |
| ![](./doc/assets/screenshots/theEndNode.png) | 流程结束节点 |

## 流式回答

`POST /flow/answer` 默认返回一个 JSON 文档；请求体带 `"stream": true` 时改为按帧推送，
响应是 `application/x-ndjson`：每行一个 JSON，`{"contentSeq": 0, "content": "..."}` 是
答案片段，最后一行必定是终帧（`contentSeq: null`），其 `content` 是完整的
`{status, data, err}` 响应。完整协议见 [`doc/streaming.md`](doc/streaming.md)。

```bash
curl -N -H 'Content-Type: application/json' \
  -d '{"robotId":"...","mainFlowId":"...","userInput":"你好","stream":true}' \
  http://127.0.0.1:12715/flow/answer
```

## 现在就开始

### 1. 准备 PostgreSQL

Kiko 需要一个可写的 PostgreSQL 数据库（开发验证用的是 18，建议 14+）。不需要 Redis、
ElasticSearch 或任何向量数据库。

```sql
CREATE DATABASE kiko;
CREATE USER kiko WITH PASSWORD 'change-me';
GRANT ALL PRIVILEGES ON DATABASE kiko TO kiko;
```

### 2. 写配置

默认读取 `data/kiko.toml`（可用 `KIKO_CONFIG` 指定其他路径）。**首次启动时如果文件
不存在，程序会生成一份带注释的模板并退出**，填好连接串后再次启动即可：

```toml
[database]
# 也可以只设置 KIKO_DATABASE_URL（优先级高于本文件），但配置文件本身必须存在
url = "postgres://kiko:change-me@127.0.0.1:5432/kiko"
# 启动时自动执行内嵌 migration（`--migrate-only` 在 S5 规划中）
auto_migrate = true

[security]
# 留空则首次启动自动生成 data/master.key（Unix 权限 0600）。
# 也可以用 KIKO_MASTER_KEY 注入。
master_key = ""

[vector]
# 所有已载入索引的内存总预算（MB），0 = 使用默认 512
index_budget_mb = 512
# 后台对账周期（秒），0 = 关闭
reconcile_secs = 60
```

### 3. 运行

```bash
kiko                          # 默认 127.0.0.1:12715
kiko -ip 0.0.0.0 -port 8888   # 指定监听地址/端口
```

首次启动会：连接 PG → 执行 migration → 生成 `data/master.key`（如果没配置）→
起 HTTP 服务。浏览器打开 `http://127.0.0.1:12715/` 即可。

> 预编译包与 Docker 镜像的发布流程属于 S5，目前尚未固化。当前从源码构建：先在
> `frontend/` 产出前端资源、放进 `src/resources/assets/`，再 `cargo build --release`
> （这条链路属于 S5 的打包项，尚未自动化）。

## 配置项与覆盖

优先级：**环境变量 > `data/kiko.toml` > 内置默认**。

| 配置 | 文件键 | 环境变量 | 说明 |
| --- | --- | --- | --- |
| 配置文件路径 | — | `KIKO_CONFIG` | 默认 `data/kiko.toml` |
| 数据库连接串 | `[database] url` | `KIKO_DATABASE_URL` | 必填（或由 env 提供） |
| 自动迁移 | `[database] auto_migrate` | — | 默认 `true` |
| 主密钥 | `[security] master_key` | `KIKO_MASTER_KEY` | 留空自动生成 `data/master.key` |
| 向量索引预算 | `[vector] index_budget_mb` | `KIKO_VECTOR_INDEX_BUDGET_MB` | MB，0 = 512 |
| 后台对账周期 | `[vector] reconcile_secs` | `KIKO_VECTOR_RECONCILE_SECS` | 秒，0 = 关闭 |

## 开发

```bash
cargo check                     # 快
cargo test                      # 单元测试
KIKO_TEST_DATABASE_URL=postgres://postgres:password@127.0.0.1:5432/kiko_dev cargo test
cargo clippy --all-targets
cargo fmt --check
```

前端源码在 `frontend/`：

```bash
cd frontend && pnpm install && pnpm dev   # http://localhost:5173，接口指向 127.0.0.1:12715
```

> 只改前端源码时不要手动改 `src/resources/assets/` 或 `src/web/asset.rs` —— 它们是
> 构建产物。前端产物内嵌进二进制的流程属于 S5 打包项，尚未自动化。

## 文档

* [`doc/streaming.md`](doc/streaming.md) —— `/flow/answer` 流式协议。
* [`README.md`](README.md) —— 上游英文 README（尚未随 Kiko 同步，以本页为准）。

## 许可证

Apache-2.0，见 [`LICENSE`](LICENSE)。
