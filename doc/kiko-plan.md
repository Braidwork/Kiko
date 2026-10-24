# Kiko（项目 B）：设计方案与实施计划

> 状态：设计定稿，实施未开始。**本文是 Kiko 唯一的权威文档**（原「存储层迁移执行步骤」
> 已于 §15–§18 合并进来）。
> 日期：2026-10-17 起草评审；2026-10 合并执行步骤并补 D33–D41
> 项目关系：
> - **A** = 现有 `dialogflowai`（`dialogflow-rs`），单二进制 + redb/turso
> - **B / Kiko（暂定名）** = 本文档目标项目：面向企业私有化部署，PostgreSQL，复用现有前端与业务逻辑
> - **C** = 未来信创/政企版本，基于 B 再演进
>
> **怎么读这份文档（给实现者 / AI agent）**
>
> - §1–§5 是「什么是对的」：目标与非目标、A 的现状、决策 D1–D41、目标架构、schema。
>   **改这里的结论要先确认。**
> - §6–§8 是机制与影响面：并发/向量/嵌入/认证/审计、前端与 SDK 改动、代码改造范围。
> - §9–§14 是行动项 B-xx、风险、验证策略、待定、明确不做。
> - §15–§18 是「先做什么、怎么算做完」：执行顺序与阶段门、从 A 带过来的坑、
>   复制到 Kiko 的清单、修订记录。
> - 开工顺序：先看 §15.1 进度表 → §15.2 阶段总览 → 要动的模块查 §5（schema）
>   与对应 B-xx 的验收标准。

---

## 1. 目标与非目标

### 目标

- 单二进制部署，外部 PostgreSQL 作为唯一事实来源。
- 数据对客户 DBA 可见、可备份、可审计；不再使用嵌入式黑盒数据库。
- 复用 A 的前端页面与 HTTP 契约，尽量减少前端改动。
- 保留 A 的 AI 推理、流程引擎、意图识别等业务逻辑。
- v1 提供基本的本地管理员登录；为 SSO、RBAC、文档级权限预留结构与接口。

### 非目标

- 不做多租户 SaaS。
- 不迁移 A 的既有数据（redb/turso → PG）；B 按新库启动。
- 不重写前端页面结构。
- v1 不实现完整 RBAC 授权、文档级 ACL、HA/多实例、ES/Milvus 适配。
- 不为 C 的信创数据库做具体适配，只保持 SQL 可移植。

### 成功标准（v1，按 D42 收敛）

- 全新 PG 实例 + 单二进制可以完成：建库/迁移（或 DBA 预跑 migration）、创建管理员、登录、
  创建机器人、配置模型、导入文档、检索问答。
- 流式应答协议 (`flow/answer`) 与前端/SDK 现有契约不破坏。
- 核心存储路径（CRUD、向量写入、事务、重建续跑）有自动化测试覆盖。
- 规模：单集合 10^5 向量下检索 P95 与内存可接受（10^6 的边界见下）。

  > **注意（与 D35 的边界）**：v1 的向量实现是**内存暴力扫描**，单集合 10^5 条是舒适区；
  > 单集合真到 10^6 时，f32 常驻约 4 GB/集合。所以 10^6 量级**尚未被 v1 的实现验证过**
  > ——验收时要分两档记录：10^5（v1 必过）、10^6（需降精度 f16/i8 或分片，或按 B-25 的
  > 结论引入 ANN）。

---

## 2. 现状盘点（基于 A 的代码实测）

| 项目 | 现状 |
| --- | --- |
| 后端规模 | 约 20,043 行 Rust |
| `ai/` | 6,956 行，模型推理/对话，基本不涉及存储 |
| `flow/` | 4,560 行，22 处 `db_executor!`（mainflow 7、subflow 9、context 5、convertor 1） |
| `man/` | 2,187 行，设置、向量报告、重建 |
| `intent/` | 1,595 行；`crud.rs` 21 处 `db_executor!`，未编译的 `crud2.rs` 另有 21 处；`phrase.rs` 6 处 `conn()` |
| `kb/` | 1,131 行，`qa.rs` 7 处 `conn()`、`doc.rs` 9 处 `conn()`，几乎全是 turso 向量 SQL |
| 主业务库 | **redb** `data/flow.dat`，键值 + JSON blob |
| 向量库 | **turso** 三个文件 `qa.dat` / `doc.dat` / `phrase.dat` |
| redb 访问点 | 92 处，其中 21 处在未编译的 `intent/crud2.rs`，实际约 71 处 |
| turso 访问 | 3 个主文件，约 82 条裸 SQL，22 处 `conn()` |
| redb 抽象 | `src/db/mod.rs` 9 个通用函数 + `db_executor!` 宏 |
| 测试 | 128 个 `#[test]` / `#[tokio::test]`，约 12 个与 DB 相关；**核心 CRUD 与向量写入路径无覆盖**；已知失败测试 `flow::rt::var_replace::tests::html_to_plain` |
| 认证/权限 | **完全不存在**（前后端均无登录、用户、角色）；CORS 当前全放开 |
| HTTP 信封 | 统一 `{status, data, err}`，HTTP 状态码恒为 200 |
| 前端 HTTP 入口 | `frontend/src/assets/tools.js` 的 `httpReq()`，无拦截器 |
| ID 现状 | 机器人 `'r' + 27` 位 = 28 字符；主流程 `count + 27` 位；QA/文档/短语为 `i64` |
| 运行时节点 | rkyv 字节（`AlignedVec`），不是 JSON |
| 设置缓存 | `src/man/settings.rs` 有进程内 `SETTINGS_CACHE`，写时更新 |
| 上传文件 | `upload_doc` 只解析 DOCX 文本，**不保存原始文件** |

> 参考文档：`doc/storage.md`（属于 A，描述 redb/turso 现状；其中依赖版本和部分计数已过时，§4.5 “不为换 RDBMS 做预留”的结论对 B 已作废）。

---

## 3. 已定决策

| # | 决策 | 说明 |
| --- | --- | --- |
| D1 | 数据库先支持 PostgreSQL | MySQL 后续通过方言隔离支持；避免 PG 专有写法 |
| D2 | 业务数据全关系化（方案 A） | 实体级拆表；canvas、settings 等文档型数据保留 JSON 列 |
| D3 | 引导配置只走本地文件 | `config.toml` + 环境变量；不再使用 redb/turso 存配置 |
| D4 | 向量是派生数据 | 事实来源在 PG；进程内索引是缓存，可随时重建 |
| D5 | 向量索引走内部 trait | v1 的具体实现见 **D35**（纯 Rust 内存暴力）；arroy / usearch 退为调研项，不再是 v1 交付物 |
| D6 | ID 策略（**2026-10-19 修订：撤回「统一纯 scru128」**） | 机器人 ID **保持** A 的 `'r'` + scru128（26 位）；主流程 ID **只去掉 `count` 前缀**（D48）。列宽统一 `VARCHAR(32)`，禁止按位数卡死。前提纠正：scru128 canonical 是 **25 位**（官方 `^[0-9A-Z]{25}$`），不是 27 |
| D7 | ANN 行 ID 与业务 ID 解耦 | 统一 `vector_items` 表，自身 `BIGINT` 主键作为索引 ID；业务实体仍用 scru128 |
| D8 | 线上 JSON 保持 camelCase | Rust 字段用 snake_case + `#[serde(rename_all = "camelCase")]` |
| D9 | 保持 `{status, data, err}` 信封 | 前端与 SDK 不动；标准 HTTP 状态码以后可叠加 |
| D10 | 删除/修改一律按 ID | 覆盖 phrase / keyword / regex / subflow；不再使用数组下标 |
| D11 | 移除内部行号泄漏 | 删除 QA 的 `vec_row_id` 与 `IntentDetail.phrase_vec_row_id` |
| D12 | 有副作用的 GET 改 POST | `mainflow/release`、`intent/phrase/regenerate-all` |
| D13 | 身份走 SSO | 优先 OIDC（Code + PKCE），后续 SAML/LDAP/OAuth2；留 `IdentityProvider` 抽象 |
| D14 | 本地引导管理员保留 | SSO 配置前必须能登录；作为 break-glass 账号 |
| D15 | 权限采用 RBAC + 资源级 ACL（结构预留） | 不引入策略引擎；v1 只实现 `Principal` 与 admin 直通 |
| D16 | 审计 v1 开启，默认关闭 | 可配置开启、可配保留期 |
| D17 | 安全审计带 hash 链 | `entry_seq` + `prev_hash` + `entry_hash` + 外部锚点 |
| D18 | 检索审计单独成表 | 高 volume，不套链，按月分区 |
| D19 | schema 迁移从第一版开始 | `sqlx::migrate!`，编译期嵌入二进制 |
| D20 | 主密钥放配置文件 | 首次启动生成，环境变量可覆盖；用于加密敏感字段 |
| D21 | 部署形态保持单二进制 | 外部 PG + 本地 `data/`（模型、上传文件） |
| D22 | 离线模型下载保持现状 | 本地 `data/models/`，不因换库改变 |
| D23 | 信创适配留给项目 C | B 只保证 SQL 可移植、方言隔离 |
| D24 | 引导管理员不使用默认口令 | 环境变量 / 配置哈希 / 首次启动随机生成三选一；禁止弱口令；首次登录强制改密 |
| D25 | v1 提供本地登录页 | 所有路由除白名单外必须认证；SSO 作为阶段 4 |
| D26 | 配置分层（已确认） | 文件只放启动前必需的引导配置；可运行时编辑的业务设置放 PG |
| D27 | v1 认证强制、RBAC 仅管理员 | 认证中间件生效；资源 ACL 表与判定代码保留但没有非管理员角色可用 |
| D28 | 向量状态按机器人隔离 | 生成状态、active model、重建任务都以 `(robot_id, collection)` 为键 |
| D29 | 运行时节点继续用 rkyv 字节 | `runtime_nodes.data BYTEA`；`contexts` 的节点状态单独用 BYTEA 列 |
| D30 | 保留原始上传文件（已确认） | 新项目保存到 `data/uploads/<robot_id>/`，记录路径、MIME、内容哈希 |
| D31 | CORS 收紧 | 私有化部署默认只允许同源/配置白名单，不再全放开 |
| D32 | 设置保留进程内缓存 | 热路径不因换 PG 变成每轮一次查询 |
| D33 | 启动即失败（fail-fast） | 配置文件缺失时生成模板后以非 0 退出；连不上库同样退出并报可操作信息；**不提供 setup 向导、不做降级启动**。UI 只能在服务起来后改配置；改 DSN 必须先试连成功再写盘，并要求重启 |
| D34 | 应用配置文件名 `data/kiko.toml` | 环境变量优先；仓库根的 `config.toml` 是 **rustc 构建配置**，两者不许重名 |
| D35 | v1 向量索引 = 纯 Rust 内存暴力实现 | 每个 `(robot_id, collection)` 一个 `Vec<f32>` 常驻内存、线性扫描 + SIMD；**无 ANN 图、无磁盘索引文件**。`VectorIndex` 用 `async fn`，分派用 `enum IndexBackend`（不用 `dyn`），给 C 阶段接 ES8/Milvus 留口。arroy/usearch 只做调研，不进 v1 |
| D36 | 敏感字段单独成表 + 引用串 | `secrets(scope, name, kind, value)`，`kind ∈ enc/env/file`；设置 JSON 里只存 `enc:…` / `env:NAME` / `file:…`，读取时 hydrate、保存时 split。**回显不得用 `***` 占位**，否则用户一保存就把引用覆盖掉 |
| D37 | 固定 schema `kiko` | 不用 `public`；连接串 `options=-csearch_path=kiko`，或每次 `SET search_path` |
| D38 | 默认关闭外联 | A 的版本检查会访问 `dialogflowai.github.io`；B 默认禁用，改成可配置项（私有化/内网环境不应主动出网，安全评测会问） |
| D39 | 不做 `kv` 桥接 | 按 B-11 一次建全表，再逐模块把调用点从 redb 切到 PG；redb 与 PG 在**同一进程内按模块共存**，直到 B-20 删掉 redb |
| D40 | `sqlx 0.9` | MSRV **1.94**；TLS 特性名 `tls-rustls`（0.8 的 `runtime-tokio-rustls` 已改名）；`default` 特性含 `any`，必须 `default-features = false` |
| D41 | 分数语义统一 | 一律用**相似度（越大越近）**：`sim = 1 - cos_distance`。现状 `intent/detector.rs:77` 已正确、`flow/rt/node.rs:1146` 是反的，迁移时统一并留对照测试 |
| D42 | v1 范围边界（**2026-10-19 重审**） | 砍出 v1 的：审计 hash 链、SSO/OIDC、RBAC（`org_units`/`roles`/`user_roles`/`resource_acl`）、`identity_providers`、`jobs` 框架、公开 ID 改造、redb 数据导出器。**全部移入阶段 7，由 C 或明确客户需求驱动**。v1 = 存储换 PG + 内存向量索引 + 最小本地管理员登录 + 打包升级 |
| D43 | 审计只做 append-only | v1 的 `audit_log` **无** `prev_hash`/`entry_hash`/`audit_chain_head`/`audit_anchor`，`entry_seq` 用 IDENTITY；`audit_access_log` **不分区**，只建 `(ts)` 索引，保留期用 `DELETE ... WHERE ts < now() - interval` |
| D44 | 单实例强制 | 启动时取 `pg_advisory_lock`（与 migration 同一把）并持有到进程退出；第二个实例启动失败。理由：内存向量索引与设置缓存无法跨进程一致，且没有告警手段 |
| D45 | 迁移与 DDL 权限 | `auto_migrate` 默认 true；置 false 时启动不跑 DDL，只校验 `schema_version`，由 DBA 跑 `--migrate-only`。migration **一律在事务内执行**，因此**不使用 `CREATE INDEX CONCURRENTLY`** |
| D46 | 会话串行化 | 同一 `(robot_id, session_id)` 的上下文读-改-写用**进程内 per-session 锁**串行化，避免两条消息并发互相覆盖（A 的 `Context::get`/`save` 是整条覆盖）。不引入乐观锁，保持最后写入胜出 |
| D47 | 重建不用 jobs 表 | 续跑 = **幂等扫描**：以 `(collection, pending_generation)` 为键，缺向量的实体就是待办；每批 32–64 行独立提交；删除实体时按 `(collection, entity_id)` 删掉所有 generation 的行，避免孤儿。进度由两条 `COUNT` 推导，不落 cursor。**无暂停/限流/跨任务历史** |
| D48 | 主流程 ID 去 `count` 前缀 | `create_main_flow` 不再 `count+1` 拼前缀，直接 `scru128::new_string()`。理由：每次创建要一次全表 `COUNT`；且前缀是字典序，10 个以上主流程时 `1x, 10x, 2x…` 的顺序本来就是错的。scru128 自身单调有序，去掉前缀反而修复排序 |

---

## 4. 目标架构

```
┌──────────────────────────────┐
│ 前端（复用 A）                │
│  Vue3 + Element Plus          │
└──────────────┬───────────────┘
               │ HTTP / NDJSON / SSE
┌──────────────▼───────────────┐
│ 单二进制（Rust + axum）       │
│                              │
│  web/        路由、Principal  │
│  service/    业务逻辑（复用）  │
│  repo/       仓储层（新增）    │
│  vector/     VectorIndex trait│
│  ai/         Candle 推理（复用）│
│  flow/       流程引擎（复用）   │
└───────┬──────────────┬───────┘
        │              │
┌───────▼──────┐ ┌─────▼────────┐
│ PostgreSQL   │ │ data/        │
│ 业务/向量/审计│ │ models/uploads│
└──────────────┘ └──────────────┘
        │
┌───────▼──────────────┐
│ 进程内索引（缓存）    │
│ v1 暴力 / 将来 ANN   │
└──────────────────────┘
```

关键原则：

- **PG 是唯一事实来源。** 向量、索引文件、内存状态都可重建。
- **仓储层隔离方言。** 业务模块不出现裸 SQL。
- **实体 + 向量同事务。** 消除 A 中跨 redb/turso 的双写不一致。
- **写操作显式并发控制。** redb 的单写者队列只串行化提交，不保护读-改-写；改用 `version` 乐观锁并审计既有竞态。

---

## 5. 数据库 Schema

### 5.1 通用约定

- **固定 schema `kiko`**（D37），不用 `public`：客户的库可能已有一堆业务表，撞名、
  `search_path` 被改、迁移误删都是现场事故。
- 公开实体主键：`VARCHAR(32)`。scru128 canonical 是 25 位；机器人 ID 带 `r` 前缀为 26 位；
  宽度留余量，**禁止按位数卡死**（A 的文档里写过 27，是错的）。
- 向量行：`BIGINT GENERATED ALWAYS AS IDENTITY`，作为 ANN 索引 ID。
- 审计序号：`BIGINT`。
- 时间：`TIMESTAMPTZ`（MySQL 用 `DATETIME(6)`）。
- 审计/创建人：`created_at`、`updated_at`、`created_by`、`updated_by`。
- 乐观锁：`version BIGINT NOT NULL DEFAULT 0`。
- JSON：PG 用 `JSONB`，MySQL 用 `JSON`；禁止 PG 专有 JSON 运算符。
- 排序：所有用户可见的列表都显式存 `position INT NOT NULL`，不依赖自增或 ID 前缀。
  - A 中子流程、关键词、正则、短语、QA 问题都靠 JSON 数组下标隐式保序；主流程靠 ID 的 `count` 前缀保序。拆成关系行后这些顺序信息会丢失，`position` 就是把它显式化。
  - 新增时 `position = COALESCE(MAX(position), -1) + 1`；删除允许留空洞；只有真正的拖拽排序才重写整组。
  - 查询一律 `ORDER BY position`，不依赖数据库的默认返回顺序。
- 外键：能连的实体一律加 FK；子表通过父表继承 `robot_id` 时，查询需 join 或冗余 `robot_id`（见各表）。
- **不再使用 `{robot_id}` 表名前缀。**

### 5.2 全局与配置

```sql
app_meta(key TEXT PRIMARY KEY, value TEXT);            -- schema_version / app_version / install_id
global_settings(id SMALLINT PRIMARY KEY DEFAULT 1, data JSONB NOT NULL,
                version BIGINT, updated_at TIMESTAMPTZ);
robots(id VARCHAR(32) PRIMARY KEY, code TEXT UNIQUE,   -- code 可选，用于可读编号
       robot_name TEXT NOT NULL, robot_type TEXT NOT NULL,
       created_at, updated_at, version);
robot_settings(robot_id VARCHAR(32) PRIMARY KEY REFERENCES robots(id) ON DELETE CASCADE,
               data JSONB NOT NULL, version BIGINT, updated_at);
```

配置分层（D26）：

- **文件/环境变量**：DB DSN、监听 ip/port、主密钥、引导管理员、日志级别。这些必须在连库前可用。
- **PG**：`global_settings` 与 `robot_settings` 中所有可由 UI 编辑的业务设置，包括模型供应商、SMTP、ASR/TTS、HF token 等；敏感字段加密。
- 当前全局设置页里的 ip/port 改为只读展示文件值，或明确提示修改配置文件后重启。

**敏感字段（D36）单独成表**，设置 JSON 里只存引用：

```sql
secrets(scope TEXT NOT NULL,      -- 'global' 或 robot_id
        name  TEXT NOT NULL,      -- chat_api_key / smtp_password / hf_token / idp_client_secret
        kind  TEXT NOT NULL,      -- enc（密文）/ env（环境变量名）/ file（密钥文件路径）
        value TEXT NOT NULL,
        updated_at TIMESTAMPTZ NOT NULL,
        PRIMARY KEY (scope, name));
```

- 写入：设置页提交时 split 出密钥字段写 `secrets`，其余写 `*.data`。
- 读取：把 `secrets` hydrate 回设置对象再返回。
- **回显必须是引用串本身**（`env:OPENAI_API_KEY`），不能用 `***` 占位——设置页是
  「整条读回 → 改 → 整条写回」，占位符会把引用覆盖掉。需要一条「读 → 原样写回 →
  库内值不变」的往返测试。
- 主密钥由 S1 的配置文件提供（首次启动生成，环境变量可覆盖）；加密字段见 §6.7。

### 5.3 流程与变量

```sql
mainflows(id VARCHAR(32) PRIMARY KEY,
          robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
          name TEXT NOT NULL, enabled BOOLEAN NOT NULL DEFAULT TRUE,
          position INT NOT NULL,
          created_at, updated_at, version);

subflows(id VARCHAR(32) PRIMARY KEY,
         robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
         main_flow_id VARCHAR(32) NOT NULL REFERENCES mainflows(id) ON DELETE CASCADE,
         name TEXT NOT NULL,
         canvas JSONB NOT NULL DEFAULT '{}'::jsonb,   -- 空流程必须是合法 JSON
         position INT NOT NULL,
         created_at, updated_at, version);

variables(robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
          var_name TEXT NOT NULL, var_type TEXT NOT NULL, var_value_source TEXT NOT NULL,
          var_constant_value TEXT, var_associate_data TEXT,
          obtain_value_expression_type TEXT, obtain_value_expression TEXT,
          connect_timeout_ms BIGINT, read_timeout_ms BIGINT, cache_enabled BOOLEAN,
          created_at, updated_at, version,
          PRIMARY KEY (robot_id, var_name));

contexts(robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
         session_id TEXT NOT NULL,
         data JSONB NOT NULL,          -- 会话其余字段
         node_state BYTEA,             -- rkyv 节点状态，不塞进 JSON
         updated_at TIMESTAMPTZ NOT NULL, expires_at TIMESTAMPTZ,
         PRIMARY KEY (robot_id, session_id));

runtime_nodes(main_flow_id VARCHAR(32) NOT NULL REFERENCES mainflows(id) ON DELETE CASCADE,
              node_id TEXT NOT NULL, data BYTEA NOT NULL,   -- rkyv
              updated_at TIMESTAMPTZ NOT NULL,
              PRIMARY KEY (main_flow_id, node_id));
```

> `variables` 以 `(robot_id, var_name)` 为业务主键，不加代理 ID：流程表达式按名字引用。改名视为“删除 + 新建”。
> `canvas` 从 A 的空字符串迁移时必须映射为 `{}`，否则 JSON 列无法写入。
>
> `runtime_nodes.data` 用 `BYTEA` 存 rkyv（D29），有两个实现约束：
> 1. PG 返回的是**未对齐**切片，而 `rkyv::from_bytes` 要求对齐 —— 读取时必须先拷进
>    `AlignedVec`（A 的 `deser_node` 已经这么做了，别当冗余删掉）。
> 2. A 在反序列化处用了 `.unwrap()`，而 release 是 `panic = 'abort'` —— 一条坏数据
>    直接杀进程（DBA 手改一个 BYTEA 就能复现）。B 必须返回 `Error`。

### 5.4 意图

```sql
intents(id VARCHAR(32) PRIMARY KEY,
        robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
        name TEXT NOT NULL, position INT NOT NULL,
        created_at, updated_at, version);

intent_keywords(id VARCHAR(32) PRIMARY KEY,
                intent_id VARCHAR(32) NOT NULL REFERENCES intents(id) ON DELETE CASCADE,
                position INT NOT NULL, keyword TEXT NOT NULL);

intent_regexes(id VARCHAR(32) PRIMARY KEY,
               intent_id VARCHAR(32) NOT NULL REFERENCES intents(id) ON DELETE CASCADE,
               position INT NOT NULL, regex TEXT NOT NULL);

intent_phrases(id VARCHAR(32) PRIMARY KEY,
               intent_id VARCHAR(32) NOT NULL REFERENCES intents(id) ON DELETE CASCADE,
               position INT NOT NULL, phrase TEXT NOT NULL, created_at);
```

### 5.5 外部 HTTP

```sql
external_http_apis(id VARCHAR(32) PRIMARY KEY,
                   robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
                   name TEXT NOT NULL, description TEXT,
                   protocol TEXT NOT NULL, method TEXT NOT NULL, address TEXT NOT NULL,
                   post_content_type TEXT, request_body TEXT,
                   content_type TEXT, user_agent TEXT,
                   created_at, updated_at, version);

external_http_params(id VARCHAR(32) PRIMARY KEY,
                     api_id VARCHAR(32) NOT NULL REFERENCES external_http_apis(id) ON DELETE CASCADE,
                     kind TEXT NOT NULL,           -- header / query / form
                     position INT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
                     value_source TEXT NOT NULL);  -- Val / Var
```

### 5.6 知识库

```sql
documents(id VARCHAR(32) PRIMARY KEY,
          robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
          file_name TEXT NOT NULL, file_size BIGINT NOT NULL,
          mime_type TEXT, original_path TEXT, content_hash TEXT,
          doc_content TEXT NOT NULL,
          created_at, updated_at, version);

doc_chunks(id VARCHAR(32) PRIMARY KEY,
           document_id VARCHAR(32) NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
           robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
           position INT NOT NULL,
           chunk_text TEXT NOT NULL,          -- 原文：会拼进 LLM prompt，不能小写化
           chunk_text_norm TEXT NOT NULL,     -- 应用层归一化（小写），只用于关键词 LIKE
           created_at);

qa(id VARCHAR(32) PRIMARY KEY,
   robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
   answer TEXT NOT NULL, created_at, updated_at, version);

qa_questions(id VARCHAR(32) PRIMARY KEY,
             qa_id VARCHAR(32) NOT NULL REFERENCES qa(id) ON DELETE CASCADE,
             robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
             position INT NOT NULL, question TEXT NOT NULL, is_primary BOOLEAN NOT NULL,
             created_at);
```

原始上传文件：

- 路径：`data/uploads/<robot_id>/<document_id>.<ext>`。
- `original_path` 存相对路径；`content_hash` 用 SHA-256，便于去重与备份校验。
- 删除文档时同步删除文件；purge 机器人时删除整个目录（A 中这段逻辑被注释掉了，B 要恢复）。

### 5.7 向量

统一成一张表，ANN 索引 ID 与业务 ID 解耦：

```sql
vector_items(
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,   -- ANN 索引 ID
    robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
    collection TEXT NOT NULL,                              -- phrase / qa / doc
    entity_id VARCHAR(32) NOT NULL,                        -- 对应实体 ID
    generation BIGINT NOT NULL,
    vec BYTEA NOT NULL,
    UNIQUE (collection, entity_id, generation)
);
CREATE INDEX vector_items_lookup ON vector_items (robot_id, collection, generation);
```

- 每行向量只带 `generation`，不重复存模型字符串。
- 重建期间新旧 generation 共存；查询只认 `active_generation`。
- `vector_items.id` 就是 `VectorIndex` 使用的 `u64`（实现内部可映射到 u32，见 6.3）。
- 删除实体时 FK 级联删除向量行；索引侧通过 dirty/对账清理。

```sql
embedding_models(id SMALLINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
                 provider TEXT NOT NULL, model TEXT NOT NULL, dimension INT NOT NULL,
                 normalized BOOLEAN NOT NULL, fingerprint TEXT NOT NULL UNIQUE, created_at);

vector_collection_state(
    robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
    collection TEXT NOT NULL,
    active_generation BIGINT NOT NULL DEFAULT 0,
    active_model_id SMALLINT REFERENCES embedding_models(id),
    pending_generation BIGINT,
    status TEXT NOT NULL DEFAULT 'idle',   -- idle / rebuilding / failed
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (robot_id, collection)
);

embedding_generations(
    robot_id VARCHAR(32) NOT NULL REFERENCES robots(id) ON DELETE CASCADE,
    collection TEXT NOT NULL,
    generation BIGINT NOT NULL,
    model_id SMALLINT NOT NULL REFERENCES embedding_models(id),
    status TEXT NOT NULL,                   -- pending / running / done / failed
    started_at, finished_at,
    PRIMARY KEY (robot_id, collection, generation)
);
```

> 嵌入模型目前按机器人配置（`src/man/settings.rs`），因此 active model、generation、重建任务都必须带 `robot_id`。`embedding_models` 只存模型定义，可全局共享。

### 5.8 身份与审计（v1 裁减版，见 D42/D43）

v1 只有**本地管理员**这一种身份；组织/角色/ACL/外部 IdP 的 DDL 见 §5.10（C 阶段）。

```sql
users(
    id VARCHAR(32) PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    display_name TEXT, email TEXT,
    password_hash TEXT NOT NULL,             -- argon2id
    status TEXT NOT NULL DEFAULT 'active',
    must_change_password BOOLEAN NOT NULL DEFAULT FALSE,
    created_at, updated_at, last_login_at
);

sessions(
    id VARCHAR(32) PRIMARY KEY,
    user_id VARCHAR(32) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,        -- 只存哈希，不存原始 token
    created_at, expires_at, last_seen_at, revoked_at,
    ip TEXT, user_agent TEXT
);

-- append-only，无 hash 链（D43）
audit_log(
    entry_seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id VARCHAR(32) NOT NULL,
    ts TIMESTAMPTZ NOT NULL,
    actor_user_id VARCHAR(32), actor_name TEXT NOT NULL,
    action TEXT NOT NULL, resource_type TEXT, resource_id TEXT,
    result TEXT NOT NULL, ip TEXT, user_agent TEXT,
    detail JSONB
);
CREATE INDEX audit_log_ts ON audit_log (ts);

-- 不分区，靠 (ts) 索引按月 DELETE（D43）
audit_access_log(
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ts TIMESTAMPTZ NOT NULL,
    actor_user_id VARCHAR(32), robot_id VARCHAR(32), collection TEXT,
    query_text TEXT, result_count INT, latency_ms INT, ip TEXT
);
CREATE INDEX audit_access_log_ts ON audit_access_log (ts);
```

- **v1 权限边界**：认证强制，但只有 `admin` 一个角色（**没有角色表**），已登录用户等价于管理员。
- **审计默认关闭**（D16）；开启后只追加、不改不删；保留期清理 = 按 `ts` 范围 `DELETE`。
- 审计链（`prev_hash`/`entry_hash`/`audit_chain_head`/`audit_anchor`）与锚点校验工具移到 C（§5.10、B-61）。
- “知识库”目前与机器人 1:1，v1 的资源类型用 `robot`；不新造 KB 实体。将来一个机器人多个库时再加 `knowledge_bases` 表（届时属 C 或大客户需求）。

### 5.9 后台任务（v1 不建，见 D42/D47）

v1 **不建 `jobs` 表**：重建续跑靠幂等扫描（D47），进度由 `vector_collection_state` +
两条 `COUNT` 推导。`jobs` 表与状态机留给 C 阶段（§5.10）。

### 5.10 C 阶段再建的 schema（v1 不建）

以下对象从 v1 移出，**不是废弃**，是排到 C 或明确客户需求：

```sql
-- 组织 / 角色 / ACL（D42）
org_units(id VARCHAR(32) PRIMARY KEY, parent_id VARCHAR(32), name TEXT NOT NULL, path TEXT NOT NULL);
roles(id VARCHAR(32) PRIMARY KEY, code TEXT NOT NULL UNIQUE, name TEXT NOT NULL, builtin BOOLEAN NOT NULL);
user_roles(user_id VARCHAR(32), role_id VARCHAR(32), PRIMARY KEY (user_id, role_id));
resource_acl(id VARCHAR(32) PRIMARY KEY,
             resource_type TEXT NOT NULL,     -- robot / knowledge_base(预留) / document(预留)
             resource_id TEXT NOT NULL,
             principal_type TEXT NOT NULL,    -- user / role / org_unit
             principal_id TEXT NOT NULL,
             permission TEXT NOT NULL);       -- read / write / admin

-- 外部 IdP（D42；SSO 见 B-63）
identity_providers(
    id VARCHAR(32) PRIMARY KEY, kind TEXT NOT NULL, issuer TEXT, client_id TEXT,
    client_secret_encrypted TEXT, enabled BOOLEAN NOT NULL DEFAULT FALSE,
    config JSONB, created_at, updated_at
);

-- 审计 hash 链（D43）：v1 的 audit_log 只保留前缀字段，链字段在此追加
ALTER TABLE audit_log ADD COLUMN prev_hash BYTEA, ADD COLUMN entry_hash BYTEA;
audit_chain_head(id SMALLINT PRIMARY KEY DEFAULT 1, entry_seq BIGINT NOT NULL, entry_hash BYTEA NOT NULL);
audit_anchor(entry_seq BIGINT PRIMARY KEY, entry_hash BYTEA NOT NULL, signature BYTEA NOT NULL, created_at);

-- 后台任务（D47）
jobs(id VARCHAR(32) PRIMARY KEY,
     robot_id VARCHAR(32) REFERENCES robots(id) ON DELETE CASCADE,
     kind TEXT NOT NULL, status TEXT NOT NULL,
     progress JSONB, cursor TEXT, error TEXT,
     created_at, started_at, finished_at, created_by);
```

> 链式审计一旦启用，`audit_log` 的写入就变成串行点（每条依赖前一条的 hash），
> 且需要 verify/repair 工具与外部锚点才有意义——这是 C 阶段独立设计的一块，不要在 v1 里半做。

---

## 6. 关键机制

### 6.1 仓储层与方言隔离

- 新增 `src/repo/`：robot、intent、flow、kb、settings、auth、audit、vector。
- 仓储方法接收/返回领域 DTO，隔离 SQL、行映射、方言差异。
- 业务模块不直接持有 `sqlx::Pool`。
- 方言差异清单：upsert、`RETURNING`、`NOW()`、BLOB/JSON 类型、分页语法、标识符引用。
- 迁移目录：`migrations/pg/`（v1），`migrations/mysql/`（后续）。
- **不要给每个聚合定义 `trait XxxRepo` + `PgXxxRepo` 实现**：直接 `repo::intent::get(...)`
  这类自由函数即可。`repo/` 的价值是「SQL 只在一处 + 可测试」，不是「可换数据库」。
  真要支持 MySQL 时再抽，那是一次机械重构；现在就抽只会让每个 repo 多一层空壳，
  而且会诱发最小公分母 SQL。

### 6.2 并发写入与设置缓存

- **redb 的单写者队列只串行化提交，不保护读-改-写。** A 中已经存在丢失更新的窗口：
  - `subflow::new_subflow`
  - `add_keyword` / `remove_keyword`
  - subflow 保存/删除（按数组下标）
  - `settings::stamp_embedding_index`（每次向量写入后读改写整个 settings）
- 所有可编辑实体带 `version`，更新使用乐观锁；冲突返回明确的业务错误。
- `contexts` 高频写，使用最后写入胜出 + `updated_at`，不做乐观锁。
- **设置缓存**：保留进程内缓存（key = robot_id），写操作后更新；否则每次对话/嵌入都会多一次 PG 查询。缓存失效点要覆盖多实例场景（v1 单实例，但要留 `updated_at` 校验）。
- 批量导入/重建分批提交，避免长事务。

### 6.3 向量索引

```rust
pub type ItemId = u64;   // 先统一别名；arroy 实际可能是 u32，spike 时确认

pub trait VectorIndex: Send {
    fn upsert(&mut self, items: &[VectorItem]) -> Result<()>;
    fn delete(&mut self, ids: &[ItemId]) -> Result<()>;
    fn search(&self, query: &[f32], limit: usize) -> Result<Vec<ScoredId>>;
    fn clear(&mut self) -> Result<()>;
    fn stats(&self) -> Result<IndexStats>;
}
```

- `ItemId` 来自 `vector_items.id`。
- **score 统一为“越大越相似”（D41）**。A 有两套方向：意图用 `1 - distance >= threshold`（已正确），QA 与文档检索用 `distance <= recall_distance`（反的）。三处都要统一，否则行为静默改变。
- 每个 `(robot_id, collection)` 一个索引实例，配 `RwLock` 或单写线程。
- v1 不把元数据过滤放进 trait；先多取 top-k，再回 SQL 过滤。
- 进程内索引按 `(robot_id, collection)` 懒加载，LRU 卸载；**v1 不落磁盘索引文件**（D35）——永远是「从 PG 重建」而不是「从磁盘加载」。
- 一致性：先写 PG，后更新索引；失败标 dirty，启动时和后台对账重建。
- **arroy / usearch spike 只做调研，不进 v1 交付**（D35）：需要确认 item ID 类型（u32/u64）、量化能力、增量删除、单集合 10^6 下的建索引时间/内存/P95。结论只影响「将来是否引入 ANN」，不影响 v1 的验收。
- **v1 不引第三方 ANN（D35）**：先用内存暴力实现——每个 `(robot_id, collection)` 一个
  `Vec<f32>`，查询线性扫描 + SIMD 内积，没有图结构也没有磁盘索引文件。
  内存 = `N × dim × 4B`（10 万条 × 1024 维 ≈ 400 MB）；超预算就按 `(robot, collection)`
  懒加载 + LRU，或把 `vector_items.vec` 存 f16（BYTEA 格式自定，改精度不动 schema）。
  trait 用 `async fn`，分派用 `enum IndexBackend`，给 C 阶段接 ES8/Milvus 留口。

### 6.4 嵌入模型与重建

- 模型设置按机器人（现状）；`active model` 也是按机器人。
- 换模型：创建新 `generation` → 后台重建 → 该机器人所有集合完成后翻转 active model。
- 每行 `generation` 是续跑的基础：待办 = 该集合里还没有 `pending_generation` 向量的实体
  （幂等扫描，D47）；进度由 `vector_collection_state` + 两条 `COUNT` 推导，**不落 progress 行**。
- 暂停/续跑、限流、失败重试列为独立专项，但 schema 先支持。
- **查询嵌入必须使用 active model**，不能使用配置里尚未生效的新模型。

### 6.5 认证与权限

- 认证中间件覆盖所有路由，白名单：登录、健康检查、静态资源、`version.json`、`check-new-version.json`。
- v1 只有 `admin` 角色；`Principal` 提取器返回真实登录用户。
- `resource_acl` 表与判定函数保留，但不强制；等 SSO/RBAC 阶段启用。
- 本地引导管理员：首次启动 `users` 为空时创建，`must_change_password=true`。
- 管理员改密前，只允许本机回环访问（或要求初始化文件中的密码），避免安装后暴露窗口。
- SSO：OIDC（Code + PKCE），用户 JIT 创建，会话用服务端 `sessions` 表。

### 6.6 审计

- 安全事件与业务写同事务写 `audit_log`。
- 检索审计写 `audit_access_log`，异步批量，可开关。
- `entry_hash = H(canonical(entry) || prev_hash)`，串行写入。
- 定期生成 `audit_anchor`（HMAC 主密钥），提供 `audit verify` 命令。
- 无外部锚点时，链只能发现疏忽性篡改；锚点日志是必需的。
- 当前有副作用的 GET（`mainflow/release`、`regenerate-all`）在改 POST 之前也不能被缓存/预取，迁移期要注意。

### 6.7 配置与密钥

```toml
[database]
url = "postgres://..."
# 密码由 KIKO_DB_PASSWORD 覆盖

[server]
ip = "127.0.0.1"
port = 12715

[bootstrap]
admin_username = "admin"
# 三选一，优先级从高到低：
# 1. 环境变量 KIKO_BOOTSTRAP_ADMIN_PASSWORD
# 2. admin_password_hash（由 `kiko hash-password` 生成）
# 3. 都不配置 → 首次启动生成随机密码，写入 data/initial_admin_password.txt
# 无论哪种方式，首次登录都必须改密；改密后删除初始化文件。

[security]
master_key = "..."   # 首次启动生成；用于加密敏感字段
```

- 加密字段：Chat / Embedding / ASR / TTS API Key、SMTP 密码、HF 下载 token、IdP client_secret。
- 主密钥与密文同机存放不能防住拿到整机的人，但能防住数据库备份泄露；交付文档要写清楚边界。

### 6.8 升级与迁移策略

- 部署形态：单个二进制 + 外部 PostgreSQL；升级 = 停服务 → 备份库 → 换二进制 → 启动迁移。
- **区分两个版本号**：
  - `schema_version`：已应用的最大 migration。
  - `compat_version`：二进制能工作的最低 schema 版本。expand 阶段不抬高 compat 下限，contract 阶段才抬高。
- DB `compat_version` 高于二进制支持范围 → 拒绝启动；低于当前 → 自动迁移或 `--migrate-only`。
- migration 只前进、不回滚；回滚通过数据库备份恢复。
- 结构性 DDL 启动时同步执行；长耗时回填写入 `jobs`，后台执行，支持进度、重试、断点续跑。
- 数据形态变化采用 expand/contract：加列 → 双写 → 回填 → 删旧列；回填期间旧二进制可工作。
- 多实例用 PG advisory lock 串行化 migration；v1 单实例也保留。
- 提供 `--migrate-only`、`--check-schema`。
- 初始化安全：管理员改密前只允许本机回环访问，或必须提供初始化文件中的密码。

---

## 7. 前端与 SDK 改动清单

| 改动 | 影响 | 说明 |
| --- | --- | --- |
| 公开 ID 改为字符串 | 文档、QA、意图、短语、子流程等 | 不再有数字 ID；机器人 ID 长度从 28 变 27 |
| 移除 `vec_row_id` / `phrase_vec_row_id` | QA、意图详情 | 删除字段与回传 |
| keyword/regex 改为对象数组 | 意图详情 | `[{id, keyword}]` / `[{id, regex}]` |
| 删除/保存按 ID | 意图详情、子流程 | 不再传数组下标 |
| `IntentDetail` 改 camelCase | 意图详情 | 与其余接口一致 |
| 有副作用的 GET 改 POST | 流程发布、短语批量重建 | `httpReq` 调用方式调整 |
| 登录页 + 路由守卫 | 新增 | v1 本地管理员登录 |
| 用户/角色管理页 | 新增 | 阶段 3/4 |
| IdP 配置页 | 新增 | 阶段 4，前端需预留工作量 |
| `tools.js` 注入认证 | 一处 | 若用 Cookie 会话则自动携带；若用 header 则注入 |
| 全局设置页的 ip/port | 调整 | 改为只读展示文件值，或提示改配置后重启 |

SDK：`flow/answer` 的流式协议与信封不变，原则上不需要改；若响应字段变化，同步 `sdk/javascript`、`sdk/java` 与 `doc/streaming.md`。

---

## 8. 代码改造范围

### 复用

- `src/ai/`：模型推理、对话生成（约 6,956 行）。
- `src/flow/` 的执行逻辑与 `src/intent/` 的匹配算法。
- `src/web/` 路由框架、`{status,data,err}` 信封、流式输出。
- 前端页面与组件（除第 7 节列出的改动）。

### 重写

- `src/db/`：redb 队列与 KV 门面整体替换为 `repo/`。
- `src/kb/qa.rs`、`src/kb/doc.rs`、`src/intent/phrase.rs`：turso 裸 SQL → 仓储方法。
- `src/man/vector_report.rs`：从 BLOB 长度推断维度 → 读元数据表。
- `src/man/settings.rs`：设置持久化 + 缓存。
- `src/man/reindex.rs`：改为基于 `jobs` + generation 的可续跑任务。
- `src/result/mod.rs`：移除 `turso::Error` / `DbBusy`，新增唯一约束、死锁、序列化失败映射。
- CRUD 调用点：约 71 处 redb 访问 + 3 个 turso 文件（22 处 `conn()`）。

### 删除依赖

- `turso`
- `redb`

---

## 9. 行动项

> 规模：S ≈ 1 天内，M ≈ 2–4 天，L ≈ 1–2 周。估算仅用于排序，不是承诺。
> 依赖列只标关键前置。

### 阶段 0：项目骨架与基础设施

| ID | 行动项 | 验收标准 | 规模 | 依赖 |
| --- | --- | --- | --- | --- |
| B-01 | 新建 Kiko 仓库与模块骨架；先让最小二进制连上 PG | 空壳服务可启动、可连接 PG；业务模块分批迁入 | M | — |
| B-02 | 配置加载：TOML + 环境变量覆盖 + 优先级 | 缺 DSN 时启动失败且信息清晰 | S | B-01 |
| B-03 | `migrations/pg/` + 启动迁移 + advisory lock | 空库自动建表；重复启动幂等 | S | B-02 |
| B-04 | `schema_version` / `compat_version` 双版本校验 | 不兼容的降级启动被拒绝 | S | B-03 |
| B-05 | `--migrate-only` / `--check-schema` | 可离线迁移与预检 | S | B-03 |
| B-06 | 主密钥生成/加载 + 加解密工具 | API Key 等字段可加密落库、可解密 | S | B-02 |
| B-07 | `hash-password` CLI + 引导管理员 + 随机密码文件 | 空库启动后可登录；无默认弱口令；强制改密 | M | B-03 |
| B-08 | `repo/` 层骨架 + 错误映射 | 无 `turso::Error`；唯一约束/死锁语义清晰 | M | B-01 |
| B-09 | CI 门禁 + 处理既有失败测试 | `fmt`/`clippy`/`test` 可绿；`html_to_plain` 修复或隔离 | S | B-01 |
| B-10 | PG 集成测试基础设施 | Docker/testcontainers 或 `KIKO_TEST_DATABASE_URL`；测试可复现 | M | B-01 |

### 阶段 1：核心实体与 schema

| ID | 行动项 | 验收标准 | 规模 | 依赖 |
| --- | --- | --- | --- | --- |
| B-11 | 初始 migration：全局、机器人、流程、变量、意图、HTTP、KB、向量、身份、审计、任务 | 空库一次迁移建全表；FK/索引/唯一约束齐全 | L | B-03 |
| B-12 | robots / robot_settings / global_settings 仓储（ID 去前缀、code 可选） | 机器人接口行为不变；设置往返一致 | M | B-11 |
| B-13 | variables 仓储（自然键、FK、改名语义） | 现有变量 CRUD 行为不变 | M | B-11 |
| B-14 | intents / keywords / regexes / phrases 仓储；按 ID 操作；position | 意图详情读写一致；并发下不会删错行 | L | B-11 |
| B-15 | mainflows / subflows 仓储；canvas 合法 JSON、position、按 ID 操作 | 画布保存/加载往返一致；空流程合法 | M | B-11 |
| B-16 | external_http_apis + params 仓储 | 三类参数顺序与来源字段往返一致 | M | B-11 |
| B-17 | 乐观锁统一实现 + 既有读改写竞态审计 | 并发更新只有一个成功；列出并修复 A 的 RMW 窗口 | L | B-12..B-16 |
| B-18 | 设置进程内缓存 + 失效策略 | 对话/嵌入热路径无额外查询；写后立即生效 | M | B-12 |
| B-19 | 仓储集成测试（核心实体 CRUD、事务、迁移） | 自动化覆盖；CI 可运行 | M | B-11 |
| B-20 | 迁移剩余 redb 调用点，删除 redb 依赖 | `cargo check` 通过；无 redb 残留 | L | B-12..B-18 |

### 阶段 2：知识库与向量

| ID | 行动项 | 验收标准 | 规模 | 依赖 |
| --- | --- | --- | --- | --- |
| B-21 | documents / doc_chunks / qa / qa_questions 仓储 + 原始文件持久化 | 上传、编辑、删除、列表与 A 一致；原文件可找回 | L | B-11 |
| B-22 | `vector_items` + generation + 实体/向量同事务 | 不出现“有记录无向量”或相反；旧 generation 可清理 | M | B-21 |
| B-23 | `embedding_models` + 按机器人 collection state；active/configured 分离 | 多机器人各自换模型互不影响 | M | B-22 |
| B-24 | `VectorIndex` trait + `ItemId` 别名 + 单测 | trait 覆盖 upsert/delete/search/clear | M | — |
| B-25 | arroy spike（**调研，非 v1 交付**） | 记录 ID 类型、量化、建索引时间、内存、P95、增量删除；结论只用于决定「将来是否引入 ANN」 | M | B-24 |
| B-26 | usearch mmap 备选 spike（**调研，非 v1 交付**） | 同 B-25 指标，作为对比 | S | B-24 |
| B-27 | 按 `(robot_id, collection)` 懒加载 + LRU（**v1 无磁盘索引文件**） | 冷启动不加载全量；内存超预算时先降精度（f16/i8），再考虑 ANN | L | B-24 |
| B-28 | dirty 标记与后台对账重建 | 索引写失败后自动恢复一致 | M | B-22, B-27 |
| B-29 | 重写 vector_report | 维度/行数来自元数据，不扫 BLOB | S | B-23 |
| B-30 | 移除 `vec_row_id` 与 `phrase_vec_row_id` | 前后端契约一致 | S | B-21 |
| B-31 | 分数语义统一（D41） | 把三处调用点统一成相似度：`intent/detector.rs:77`（已正确）、`flow/rt/node.rs:1146` 的 QA 与文档检索（现在是 `distance <=`，要翻转）；有对照测试 | M | B-24 |
| B-32 | 文档检索保留向量 + SQL 关键词的 RRF 融合 | 行为与 A 一致，有对照测试 | M | B-21 |

### 阶段 3：认证、权限、审计

| ID | 行动项 | 验收标准 | 规模 | 依赖 |
| --- | --- | --- | --- | --- |
| B-33 | `Principal` 提取器 + 认证中间件 + 路由白名单 | 除白名单外未认证请求被拒绝 | M | B-08 |
| B-34 | 本地登录页、路由守卫、会话表（token 哈希） | 管理员可登录；会话可吊销；未改密前限制访问 | L | B-33 |
| B-35 | 权限判定：v1 只有 admin（**不建角色表**） | 已登录即管理员；未认证路径有明确错误 | S | B-34 |
| B-39 | 敏感配置加密接入（Chat/Embedding/ASR/TTS/SMTP/HF） | 数据库中无明文 | S | B-06 |
| B-40 | CORS 收紧与安全头 | 默认只允许同源/白名单 | S | B-33 |
| B-41 | 登录/会话接口测试 | 认证绕过、过期、吊销都有用例 | M | B-34 |

> B-36 / B-37 / B-38（审计 hash 链、锚点、检索审计分区）与 B-42（审计覆盖清单）
> 已移出 v1，见阶段 7 的 B-61 与 D42/D43。

### 阶段 4：SSO —— **已推迟到 C**

v1 不做 SSO（D42）。本地管理员是唯一登录方式；`users.idp_kind` 这类字段等 C 阶段
按 §5.10 追加。原 B-43…B-46 见阶段 7 的 B-63。

### 阶段 5：后台任务 —— **已推迟到 C**

v1 不建 `jobs` 表（D42/D47）。重建续跑靠幂等扫描（§6.4），进度由
`vector_collection_state` 与 `COUNT` 推导。原 B-47…B-50 见阶段 7 的 B-64。

### 阶段 6：打包与升级

| ID | 行动项 | 验收标准 | 规模 | 依赖 |
| --- | --- | --- | --- | --- |
| B-51 | 单二进制打包（前端产物、migration 内嵌） | 目标机无需 Rust/Node | M | B-03 |
| B-52 | 健康检查含 schema/app/compat 版本 | 运维可判断升级状态 | S | B-04 |
| B-53 | 升级文档：备份、停服、换二进制、迁移、回滚边界 | 可按文档完成升级演练 | S | B-51 |
| B-54 | 离线模型下载回归验证 | 与 A 行为一致 | S | B-01 |
| B-55 | 升级演练（空库 → 导入 → 升级 → 恢复） | 全流程通过 | M | B-53 |

### 阶段 7：后续（C / 非 v1）

| ID | 行动项 | 说明 | 规模 |
| --- | --- | --- | --- |
| B-56 | MySQL 方言支持 | 新增 `migrations/mysql/` 与方言实现 | L |
| B-57 | 信创数据库适配（金仓/openGauss/达梦） | 属项目 C，先验证 SQL 可移植性 | L |
| B-58 | ES8 / Milvus 适配 | 项目 C 或大客户驱动 | L |
| B-59 | 文档级 ACL | 与外部检索或 pre-filter 方案一起设计 | L |
| B-60 | 多实例/HA | 索引一致性、migration 锁、会话共享 | L |
| B-61 | 审计 hash 链 + 锚点 + `audit verify` | C；原 B-36/37/38/42，DDL 见 §5.10（D43） | L |
| B-62 | 组织 / 角色 / 资源 ACL | C；原 B-35 的 RBAC 部分，DDL 见 §5.10（D42） | L |
| B-63 | SSO（OIDC Code+PKCE，后续 SAML/LDAP）+ `identity_providers` | C；原 B-43…B-46 | L |
| B-64 | 后台任务框架 `jobs`（暂停/续跑/限流/回填） | C；原 B-47…B-50（D47） | L |
| B-65 | 公开 ID 统一（去掉 `r` 前缀） | C；v1 不做（D6），改动横跨前端/SDK/测试 | M |
| B-66 | 从 A 升级的数据迁移工具（导出 + 导入闭环） | C；v1 明确不做（D42）。只做导出不构成闭环，所以整块后置 | L |

---

## 10. 风险清单

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| arroy 的 ID 类型/量化/维护状态不确定 | 向量方案返工 | **v1 已绕开**（D35：内存暴力实现，不引第三方 ANN）；风险降级为「将来要不要换 ANN」的决策，届时再做 B-25 spike |
| 现有 DB 路径缺少自动化测试 | 重写引入回归且难发现 | B-10/B-19；先补核心路径 |
| A 既有读-改-写竞态被误认为“已串行化” | 换 PG 后丢失更新 | B-17 显式审计并修复 |
| 设置搬进 PG 后热路径多一次查询 | 对话/嵌入延迟上升 | B-18 缓存 |
| 1M 切片内存/启动 | 私有化服务器资源不足 | 懒加载 + LRU + 量化 + mmap；压测验收 |
| 意图分数语义反转 | 检测行为静默变化 | B-31 转换 + 回归测试 |
| 文档检索 RRF 融合被简化 | 召回质量下降 | B-32 保留融合并有对照 |
| 前端契约改动扩散 | 前端改动超出预期 | 只做第 7 节清单；保持信封不变 |
| SSO/权限范围膨胀 | 拖延核心存储改造 | 阶段 0–2 先交付可用版本 |
| expand/contract 与版本校验冲突 | 回填期间旧二进制拒绝启动 | compat_version 与 schema_version 分离 |
| 审计链被误解为“绝对防篡改” | 交付预期不符 | 文档写明锚点边界；演示 verify |
| CORS 全放开 | 私有化环境安全评测不通过 | B-40 收紧 |
| 上传原文件未持久化 | 无法重解析/下载/备份 | B-21 落盘 + 哈希 |
| 测试环境没有 Docker/PG | 集成测试不可执行 | B-10 支持外部 `KIKO_TEST_DATABASE_URL` |

---

## 11. 验证策略

1. **单元测试**：仓储映射、乐观锁、向量 trait、重建的幂等续跑。
2. **集成测试**：临时 PG（Docker/testcontainers 或外部 DSN），覆盖核心实体 CRUD、事务、迁移。
3. **契约测试**：`flow/answer` 非流式与流式（NDJSON/SSE）输出与 A 一致。
4. **行为对照**：意图检测分数、文档 RRF 检索与 A 的对照测试。
5. **规模验证**：100 万切片 + 20 QPS；记录内存、P95、冷启动。**分两档**记：
   单集合 10^5（v1 必过）；单集合 10^6（v1 是内存暴力扫描，f32 约 4 GB —— 需降精度
   f16/i8 或分片，或按 B-25 的结论引入 ANN，不能笼统写「通过」）。
6. **升级演练**：空库安装 → 导入 → 升级到下一 schema → 备份恢复。
7. **安全验收**：未登录拒绝、会话吊销、越权拒绝、审计校验、敏感字段无明文、CORS。

---

## 12. 待定问题

| # | 问题 | 当前倾向 |
| --- | --- | --- |
| O1 | 项目 B 的正式名称 | 暂定 **Kiko** |
| O2 | v1 是否必须提供登录页 | **已定：提供本地管理员登录页** |
| O3 | SSO 首个目标协议 | OIDC；本地用 Keycloak/Dex 测试 |
| O4 | 审计保留期默认值 | 安全审计 180 天，检索审计 90 天，可配置 |
| O5 | 支持的操作系统 | Windows + Linux（单二进制） |
| O6 | PG 最低版本 | 建议 14+ |
| O7 | 是否需要多实例/HA | v1 单实例 |
| O8 | 文档级 ACL 的时间点 | 项目 C 或首个明确客户需求 |
| O9 | 是否为 A 的存量用户提供迁移工具 | 当前不在范围内 |
| O10 | 离线安装包是否包含 PG | 当前假设客户提供或由 DBA 安装 |
| O11 | 文件配置与 PG 设置的分界 | **已定**：文件放 DSN/监听/主密钥/引导；PG 放可 UI 编辑的业务设置；设置页的 ip/port 改为只读展示 |
| O12 | 去掉 ID 前缀是否可接受 | **已推翻（2026-10-19）**：v1 保留 `r` 前缀（D6）；只去掉主流程的 `count` 前缀（D48）；统一 ID 属 C 阶段的 B-65 |
| O13 | 是否保留原始上传文件 | **已定**：保留（D30），落盘 + 内容哈希 |
| O14 | 1M 向量 spike 数据与测试环境 | 合成向量 + 小规模真实语料；集成测试支持外部 PG |
| O15 | arroy 的 item ID 类型 | **v1 不再阻塞**（D35 用内存暴力实现）；若 C 阶段决定引入 ANN，再按 B-25 spike 验证 ID 类型/量化/建索引耗时 |
| O16 | RBAC 是否 v1 就强制 | 建议 v1 只强制认证，RBAC 仅 admin |

---

## 13. 明确的“不做”

- 不重写 `ai/`、`flow/` 的业务逻辑。
- 不改前端页面结构，只做第 7 节列出的契约适配。
- 不为 C 提前实现信创数据库适配。
- 不为性能预优化没有真实数据支撑的路径。
- 不引入 ORM 或策略引擎；手写 SQL + 仓储层。
- 不承诺 migration 可回滚；回滚依赖备份。
- v1 不实现文档级 ACL、多实例、ES/Milvus。

---

## 14. 独立评审修正记录

本轮评审后修正的主要问题：

1. **事实计数**：`flow/` 的 `db_executor!` 22 处（原写 17）；turso `conn()` 22 处（原写 17）；测试现状改为“128 个测试，核心 CRUD/向量写入无覆盖”。
2. **ID 策略**：scru128 为 27 字符；现有机器人 ID 28 字符、主流程带计数前缀。改为纯 scru128，列宽 `VARCHAR(32)`。
3. **向量行身份**：新增统一 `vector_items`，ANN 使用自身 BIGINT ID，解决 phrase/QA 向量没有 BIGINT 身份的问题。
4. **按机器人隔离**：`vector_collection_state`、`embedding_generations`、`jobs` 全部加 `robot_id`。
5. **运行时数据**：`runtime_nodes.data` 改 `BYTEA`；`contexts` 的节点状态单独用 `BYTEA`。
6. **配置分层**：明确文件与 PG 的边界，避免 D3 与 settings schema 冲突。
7. **认证一致性**：明确 v1 强制认证、RBAC 仅 admin，解决“预留”与“强制”的矛盾。
8. **升级兼容**：拆分 `schema_version` 与 `compat_version`，修复 expand/contract 与“拒绝降级”的冲突。
9. **Schema 缺口**：补 `subflows.position`、FK、`canvas` 合法默认值、`users` 唯一约束/本地密码 CHECK、`sessions.token_hash`。
10. **已知行为差异**：列出意图分数反转、文档 RRF 融合、设置缓存、CORS、原始文件持久化等必须在实施时处理的问题。
11. **B-01 顺序**：改为先搭骨架，再分批迁入业务模块，不再要求“删掉 redb/turso 后立刻 `cargo check` 通过”。
12. **CI 基线**：把既有的 `html_to_plain` 失败测试纳入 B-09。

---

## 15. 执行顺序与阶段门

> 本节回答三件事：先做什么、每一步怎么算做完、做坏了怎么退。
> 决策与 schema 见 §3/§5；行动项编号见 §9。

### 15.1 当前进度

| 阶段 | 状态 | 备注 |
| --- | --- | --- |
| 复制到 Kiko | ☐ | 见 §17 |
| S0 仓库基线 | ☐ | 改名、sqlx、骨架、测试基建、修 A 的两个坑（§16.1/§16.2） |
| S1 配置层 | ☐ | fail-fast（D33）、`data/kiko.toml`（D34）、主密钥 |
| S2 PG 基础设施 + 全表 + 机器人/设置 | ☐ | B-03…B-08、B-11、B-12 |
| S3 逐模块切换 | ☐ | B-13…B-18，收尾条件 B-20 |
| S4 向量 | ☐ | B-21…B-32 |
| S5 收尾 | ☐ | 删依赖、契约清理、打包 |

> 每完成一步就更新这张表；状态用 ☐ / 进行中 / ☑。

### 15.2 阶段总览

| 阶段 | 目标 | 存储落点变化 | 规模 | 对应行动项 |
| --- | --- | --- | --- | --- |
| S0 | 仓库基线：去掉 A 的残留、能跑、能测、能连 PG | 不变（仍全在 redb/turso） | S | B-01, B-09, B-10 |
| S1 | 配置层：文件 + env，fail-fast 启动 | 引导配置 redb → 文件 | M | B-02, B-06 |
| S2 | PG 基础设施 + 全表 migration + 机器人/设置 | redb 开始让位 | M–L | B-03…B-08, B-11, B-12 |
| S3 | 逐模块切换 + 乐观锁 | redb → PG（逐模块） | L | B-13…B-18, B-17, B-20 |
| S4 | 向量：turso → PG + 进程内索引 | turso → PG | L | B-21…B-32 |
| S5 | 收尾：删依赖、清语义债、打包 | — | M | B-20, B-29, B-30 |

### 15.3 共存期规则

**共存期 = S2 开始 → S4 结束。**

```
S2 期间：robots/设置 = PG，其余模块 = redb，向量 = turso
S3 期间：模块逐个从 redb 搬到 PG；没搬的仍在 redb；向量仍在 turso
S4 期间：向量从 turso 搬到 PG；turso 下线
```

硬规矩：

1. 一个模块的数据**只在一个库**里，**不做双写**。
2. 跨库写（典型：`DELETE /robot` 的 purge）写成两段 + 幂等 + 可重试，允许部分成功
   并留可对账的残留。**禁止**任何「必须两库同时成功」的代码路径。
3. 数据归属矩阵：

| 数据 | S0 | S1 | S2 | S3 | S4 | S5 |
| --- | --- | --- | --- | --- | --- | --- |
| DSN / 监听 / 主密钥 / 引导管理员 | redb | **文件** | 文件 | 文件 | 文件 | 文件 |
| `robots` / `global_settings` / `robot_settings` / `secrets` | redb | redb | **PG** | PG | PG | PG |
| variables / external_http | redb | redb | redb | **PG**（先切） | PG | PG |
| intents（含 keywords/regex/phrases） | redb | redb | redb | **PG** | PG | PG |
| contexts / runtime_nodes | redb | redb | redb | **PG** | PG | PG |
| mainflows / subflows | redb | redb | redb | **PG**（最后切） | PG | PG |
| qa / doc / phrase 载荷 + 向量 | turso×3 | turso×3 | turso×3 | turso×3 | **PG + 内存索引** | PG |
| 嵌入模型 / 索引状态 | redb（settings 内） | redb | PG | PG | **PG 独立表** | PG |
| 模型文件 / 上传原文件 | `data/` | `data/` | `data/` | `data/` | `data/` | `data/` |

### 15.4 各阶段步骤

**S0 — 仓库基线**

1. 改名与清理：`Cargo.toml` 的 `name` / `[lib] name`、`src/main.rs:11` 的
   `use dialogflowai::…`、`src/main.rs:36` 线程名、`src/ai/huggingface.rs:1093` 的
   User-Agent、`.gitignore` 的 `dialogflowai.exe`、`.github/workflows/ci.yml`
   （建议直接删：它是 `workflow_dispatch` + 仅 macOS，对 B 无意义）。
2. 依赖换 `sqlx 0.9`（D40）：`default-features = false`，
   features = `runtime-tokio` / `tls-rustls` / `postgres` / `json` / `migrate` / `macros` / `derive`。
3. 建 `src/repo/`、`src/config/`、`migrations/` 骨架。
4. 集成测试基建（B-10）：`KIKO_TEST_DATABASE_URL` 优先，Docker/testcontainers 兜底；
   每个测试 `CREATE SCHEMA tmp_<uuid>` + `search_path`，结束 drop。
5. 修 §16.1 / §16.2 两个坑；按 D38 关掉版本检查外联。
6. 删除 A 的死代码（`intent/crud2.rs`、`db/embedding*.rs`、`flow/rt/*.bak`、
   `variable/crud.rs` 的 sqlx 注释块）—— 复制时若已排除，这步跳过。

验收：`cargo check --all-targets` 通过；`cargo test` 绿；集成测试能建 schema、
跑 `SELECT 1`、清理干净。

**S1 — 配置层（fail-fast，D33/D34）**

1. 加载顺序 **env > 配置文件 > 内置默认**；路径 `KIKO_CONFIG`，默认 `./data/kiko.toml`。
2. **首次运行**（文件不存在）：生成带注释的模板（`[database] url` 留空）+ 随机
   `master_key`，然后**打印错误日志并以非 0 退出**，日志写清「请填写
   data/kiko.toml 的 [database] url 后重启」。
3. 有文件但缺 DSN / 连不上 PG：打印可操作错误（主机、库名、PG 错误码），非 0 退出。
   **不 panic、不猜 DSN、不进降级模式。**
4. 写盘：临时文件 + rename；Linux `0600`，Windows 收紧 ACL；密码可用
   `KIKO_DB_PASSWORD` 覆盖。
5. UI 只能在服务起来后改配置；改 DSN 必须先试连成功再写盘，并提示重启。

验收：删配置启动 → 生成模板 + 退出码非 0；错 DSN → 日志有 PG 错误码；正确 DSN → 正常启动。

**S2 — PG 基础设施 + 全表 + 机器人/设置**

1. `migrations/0001_init.sql` 按 §5 一次建全表（含 FK、唯一约束、`position`、
   `version`、`secrets`），`sqlx::migrate!` 编译期内嵌；固定 schema `kiko`（D37）。
2. 启动：配置 → 连池 → advisory lock 串行 migration → 校验 `schema_version` /
   `compat_version`（B-04）→ 起服务；`--migrate-only`、`--check-schema`（B-05）。
3. `src/repo/` 骨架 + 错误映射（B-08）：唯一约束 → 明确业务错误；
   删除 `turso::Error` / `DbBusy` 映射。
4. **B-12**：`robots` / `global_settings` / `robot_settings` / `secrets` 仓储，
   替换 `robot/crud.rs` 与 `man/settings.rs` 的读写 —— 这是第一个「模块粒度共存」点。
5. 主密钥 + 加解密工具（B-06）。

验收：空库启动自动建全表、重复启动幂等；`POST/DELETE /robot` 正确落库与级联；
设置往返一致；混合状态下 purge 幂等。

**S3 — 逐模块切换**

| # | 模块 | 新表 | 切换时必须一起改的跨模块读者 | 规模 |
| --- | --- | --- | --- | --- |
| 3.1 | 变量 | `variables` | 仅 `robot::purge` | S |
| 3.2 | 外部 HTTP | `external_http_apis` / `_params` | 仅 `purge` | S |
| 3.3 | 意图 | `intents` / `intent_keywords` / `intent_regexes` / `intent_phrases` | `intent/detector.rs:14`、`man/reindex.rs:160`、`purge` | M |
| 3.4 | 会话 | `contexts` | `flow/rt/context.rs` 内部；`clean_expired_session` 改为 PG 查询（不再遍历机器人） | S |
| 3.5 | 运行时节点 | `runtime_nodes` | `flow/rt/convertor.rs`（写入）、`purge` | S |
| 3.6 | 流程 | `mainflows` / `subflows` | `flow/rt/convertor.rs:22`、`purge` | M |
| 3.7 | 乐观锁横切 | 全部可编辑实体 | 见 §16.4 的 RMW 清单 | M |

每模块验收模板：

1. 新 repo 集成测试：CRUD + 顺序（`position`）+ 并发两次更新只成功一次；
2. 前端对应页面走一遍（变量列表 / 意图详情+关键词 / 流程画布发布 / 多轮对话）；
3. `grep` 确认该模块的 `TABLE_SUFFIX` 在 `src/` 里已无引用；
4. 混合状态下 `DELETE /robot` 幂等。

**B-20（删 redb 依赖）是 S3 的收尾条件**：`cargo tree | grep redb` 为空。

**S4 — 向量迁移**

1. `migrations/0002_vectors.sql` 按 §5.6/§5.7；`doc_chunks.chunk_text` **必须在 PG**
   （关键词召回依赖它，见 §16.3）。
2. `VectorIndex` trait + v1 `BruteForce`（D35、§6.3）；分数统一成相似度（D41）。
3. 写：载荷表与 `vector_items` 同事务；提交后更新内存索引，失败标 dirty。
4. 读：索引取 top-k（多取一些）→ 回 PG 取载荷 → doc 保留 RRF 融合（B-32）。
5. 重建：`generation` 语义 + **幂等续跑**（B-23 / D47，不建 `jobs` 表）。
6. 关键词召回改 PG 查询：**v1 只依赖内建 `LIKE` + 一列应用层归一化文本，不要求任何扩展**。
   `doc_chunks` 同时存 `chunk_text`（原文）与 `chunk_text_norm`（Rust 侧 `to_lowercase()`
   归一化，只用于匹配）；关键词侧用同一个函数处理，两边规则一致，
   **不依赖数据库的 collation/locale**（客户的库是 `C` 还是 `zh_CN.UTF-8` 我们控制不了）。
   全表扫（单机器人 10^4–10^5 条可接受）；`pg_trgm` / `zhparser` / `bigram+GIN` 都只是
   可选加速，缺了不影响功能。
   **不要直接把 `chunk_text` 小写化**——它要拼进 LLM prompt，小写会降低答案质量。
   顺手修掉 A 的大小写 bug（`extract_keywords` 小写化但 `chunk_text` 没有）。
7. `vector_report` 改读元数据表（B-29）；`purge` 收成一次事务 + 索引 `clear()`。

验收：清空 `data/` 后建机器人 → 导入 docx → 建 QA → 建短语 → 三条检索全命中；
换模型重建后旧 generation 清理；`data/{qa,doc,phrase}.dat` 不再被创建；
100 万合成向量的 P95 / 内存 / 冷启动数据补齐（替代 B-25 的验收）。

**S5 — 收尾**

删 redb/turso 依赖与队列代码；删 `{robot_id}` 表名拼接残留（`TABLE_SUFFIX`、
`init_table`、`delete_table`、`db_executor!` 宏）；契约清理（B-30、D12、D6）；
打包（migration 与前端产物内嵌）；新写 B 版 `doc/storage.md`，A 的那份移入
`doc/legacy/` 或加「仅适用 A」头注。

### 15.5 能不能停

| 停在这里 | 后果 | 可接受 |
| --- | --- | --- |
| S1 后 | 配置已独立，业务仍在 redb/turso | 可接受（无收益也无损失） |
| S2 后 | robots/设置在 PG，其余在 redb，向量在 turso | 可接受，但这是最别扭的中间态 |
| S3 中间停 | 部分模块在 PG、部分在 redb | 可接受（前提：不再在 redb 上加功能） |
| S4 中间停 | 向量半迁移 | **不可接受**：检索行为无法解释，必须整段完成 |

---

## 16. 从 A 带过来的坑（复制时必须处理，别照抄）

### 16.1 `deser_node` 的 `unwrap` + `panic = 'abort'`

`flow/rt/node.rs:1272-1281` 用 `rkyv::from_bytes(...).unwrap()`，而 `profile.release`
是 `panic = 'abort'` → 一条坏字节直接杀进程。改成返回 `Error`。
`AlignedVec::with_capacity + extend_from_slice` 那次拷贝**必须保留**（见 §5.3 的说明）。

### 16.2 `phrase::search` 里有一段「没被注释掉」的调试查询

`intent/phrase.rs:87-119`：`//*` 与 `//*/` **都是行注释**，夹在中间的那次不带
`LIMIT` 的全表扫描（还逐行 `log::info!`）是**活的**——每次意图检测都先跑一遍它，
再跑真正带 `LIMIT 1` 的那条。S3.3 重写 phrase 仓储时删掉。

### 16.3 关键词召回真实存在，且依赖 `chunk_text` 与向量同表

链路：`flow/rt/node.rs:1160` → `kb/doc.rs::search_doc` → `keyword_recall`
（ASCII 词 + 中文 bigram → `LIKE '%kw%' ESCAPE` → 与向量榜 RRF 融合）。
它不是 FTS（无索引、无分词器），但确实在生产路径上跑；`chunk_text` 目前躺在
turso 的 `{robot_id}_vec` 表里。→ S4 必须把 `chunk_text` 搬进 PG 并重写这条召回，
否则搬完向量会**静默**丢掉一半召回能力（RRF 退化成纯向量，不报错）。

### 16.4 A 的读-改-写竞态

`add_keyword` / `remove_keyword` / `add_regex` / `remove_regex`（`intent/crud.rs`）、
`new_subflow`、`stamp_embedding_index` 都是「读整条 → 改 → 写回」。
redb 的单写队列**只串行化提交，不串行化 RMW**，所以现在就有丢更新窗口。
正解是子表按 ID 操作（D10）+ `version` 乐观锁（B-17），不要指望换 PG 自动好。

### 16.5 两套「距离」方向

见 D41 / B-31。改的时候注意设置页的 `similarity_threshold` 与 `recall_distance`
两个配置项语义也要写清，否则用户调阈值会拧反。

---

## 17. 从 A 复制到 `D:\work\Kiko` 的清单

### 不要复制

| 路径 | 大小 | 原因 |
| --- | --- | --- |
| `target/` | 24 GB | 构建产物 |
| `data/` | 1.15 GB | 其中 `*.dat` 是 A 的 redb/turso 库（B 明确不要）；`data/models` 需要离线时再单独拷 |
| `frontend/node_modules/` | — | `pnpm i` 重装 |
| `frontend/dist/` | — | 构建产物 |
| `.git/` | 12 MB | 新仓库 `git init` |
| `.idea/`、`.acl-recovery/`、`.dsh-sandbox-probe.txt` | — | IDE / 工具残留 |
| `todo.txt`、`tree.txt` | — | A 的临时笔记 |

### 要复制

`src/`、`frontend/`（除上面两项）、`sdk/`、`doc/`、`examples/`、`build.rs`、
`Cargo.toml`、`Cargo.lock`、`Cross.toml`、`musl-zcc`、`musl-zcxx`、`.cargo/`、
`rustfmt.toml`、`.gitignore`、`LICENSE`、`README*.md`、`AGENTS.md`（用 Kiko 版覆盖）。

**本文档在 Kiko 里建议重命名为 `doc/kiko-plan.md`**；`AGENTS.md` 指向它。

### 复制后必须改

| 位置 | 改什么 |
| --- | --- |
| `Cargo.toml` | `name` / `[lib] name` → `kiko`；`homepage` / `authors` |
| `src/main.rs:11` | `use dialogflowai::…` → `use kiko::…` |
| `src/main.rs:36` | 线程名 |
| `src/ai/huggingface.rs:1093` | User-Agent 里的 `dialogflowai/` |
| `.gitignore` | `dialogflowai.exe` → `kiko.exe`；补 `data/kiko.toml`、`data/initial_*` |
| `.github/workflows/ci.yml` | 改名或直接删除 |
| `r.bat` / `t.bat` / `z.bat` / `frontend/*.bat` | 建议删除，改用 AGENTS 里的命令 |
| `AGENTS.md` | 用 Kiko 版覆盖（见同目录 `AGENTS-kiko.md`） |
| `doc/storage.md` | 加「仅适用 A」头注，或移到 `doc/legacy/` |

### 注意

- 根目录的 `config.toml` 是 **rustc 构建配置**（`[rust] debuginfo-level`），不是应用
  配置；B 的应用配置是 `data/kiko.toml`（D34），别重名。
- `D:\work\Kiko` 比 `D:\work\dialogflow-rs` 短，对 `node_modules` 深层路径更友好。
- 复制完先跑一次 `cargo check`：此时依赖还是 redb/turso，能过就说明复制完整，
  再按 §15.4 的 S0 开始改。

---

## 18. 修订记录（2026-10 合并轮）

1. 合并原 `project-b-storage-migration-steps.md`（执行顺序、阶段门、A 的坑、
   复制清单）为 §15–§18，此后**只有这一份文档**。
2. 补决策 D33–D41：fail-fast 启动、`data/kiko.toml`、v1 暴力向量索引、
   secrets 表 + 引用串、固定 schema `kiko`、关闭外联、不做 `kv` 桥接、
   `sqlx 0.9`、分数语义统一。
3. 明确 **不做 `kv` 桥接**（D39）：本文早前版本考虑过「先用一张 `kv` 表接管
   54 个 `db_executor!` 调用点、再逐模块规范化」，但因为 B 没有存量数据要搬，
   收益小于「多写一遍再删掉」的成本，改为 B-11 建全表后逐模块切换。
4. `runtime_nodes.data BYTEA`（D29）补两条实现约束：保留 `AlignedVec` 拷贝、
   反序列化返回 `Error` 而非 `unwrap`。
5. B-31 从「意图检测」扩到三处调用点（意图、QA、文档检索）。
6. §5.2 增加 `secrets` 表与「引用串必须原样回显」的实现陷阱。
7. 消除与 D35 的残留矛盾：D5 改为指向 D35；B-25/B-26 标为「调研，非 v1 交付」；
   B-27 去掉「磁盘缓存」；§6.3 去掉「磁盘索引文件是缓存」的说法。
8. §1 成功标准的「100 万切片」补边界说明：v1 是内存暴力扫描，单集合 10^6 尚未验证，
   验收必须分 10^5（必过）/ 10^6（需降精度/分片/ANN）两档；§11 验证策略同步。

### 2026-10-19 重审轮（换模型复审后的范围收敛）

9. **范围收敛（D42）**：审计 hash 链、SSO/OIDC、RBAC（`org_units`/`roles`/`user_roles`/
   `resource_acl`）、`identity_providers`、`jobs` 框架、公开 ID 改造、redb 迁移工具
   全部移出 v1 → 阶段 7 的 B-61…B-66。v1 = 存储换 PG + 内存向量索引 + 最小本地管理员
   登录 + 打包升级。
10. **审计去链（D43）**：v1 的 `audit_log` 只做 append-only；`audit_access_log` 取消
    `PARTITION BY`，改 `(ts)` 索引按月 `DELETE`。链与锚点的 DDL 移入 §5.10。
11. **事实纠正（D6）**：scru128 canonical 是 **25 位**，不是 27；`VARCHAR(27)` 全部
    改为 `VARCHAR(32)`；撤回「统一纯 scru128」。
12. **新增 D44–D48**：单实例 advisory lock、`auto_migrate` 开关并禁用
    `CREATE INDEX CONCURRENTLY`、per-session 会话锁、重建幂等续跑（不建 jobs）、
    主流程 ID 去 `count` 前缀。
13. 阶段 3/4/5 的表格按上表改写；阶段 4/5 整节改为「已推迟到 C」。
