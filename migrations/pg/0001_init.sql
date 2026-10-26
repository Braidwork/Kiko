-- 0001_init.sql —— Kiko v1 初始 schema（唯一事实来源：doc/kiko-plan.md §5）
--
-- 约定：
--   * 所有对象都建在固定 schema `kiko` 下（D37），SQL 里**不写 schema 前缀**，
--     由连接的 search_path 决定落到哪里：应用连接串带 `search_path=kiko`，
--     集成测试带 `search_path=tmp_<uuid>`，因此同一个文件既能建生产库也能建测试库。
--     执行前 schema 必须已存在 —— 应用在跑 migration 之前先 `CREATE SCHEMA IF NOT EXISTS kiko`。
--   * 不要用 psql 直接跑这个文件：DBA 离线迁移请用 `kiko --migrate-only`（D45）。
--   * migration 只前进不回滚，已应用的文件不要改（sqlx 会校验 checksum）。
--   * 所有 FK 列都建索引：级联删除和按父实体查询都要走索引（§5.1）。

-- ============================================================ 5.2 全局与配置

CREATE TABLE app_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE global_settings (
    id         SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    data       JSONB NOT NULL,
    version    BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE robots (
    id         VARCHAR(32) PRIMARY KEY,
    code       TEXT UNIQUE,
    robot_name TEXT NOT NULL,
    robot_type TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version    BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE robot_settings (
    robot_id   VARCHAR(32) PRIMARY KEY REFERENCES robots (id) ON DELETE CASCADE,
    data       JSONB NOT NULL,
    version    BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 敏感字段单独成表（D36）：设置 JSON 里只存引用串（enc: / env: / file:）。
CREATE TABLE secrets (
    scope      TEXT NOT NULL, -- 'global' 或 robot_id
    name       TEXT NOT NULL, -- chat_api_key / smtp_password / hf_token / ...
    kind       TEXT NOT NULL CHECK (kind IN ('enc', 'env', 'file')),
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope, name)
);

-- ============================================================ 5.3 流程与变量

CREATE TABLE mainflows (
    id         VARCHAR(32) PRIMARY KEY,
    robot_id   VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    enabled    BOOLEAN NOT NULL DEFAULT TRUE,
    position   INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version    BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX mainflows_robot_id ON mainflows (robot_id);

CREATE TABLE subflows (
    id           VARCHAR(32) PRIMARY KEY,
    robot_id     VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    main_flow_id VARCHAR(32) NOT NULL REFERENCES mainflows (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    canvas       JSONB NOT NULL DEFAULT '{}'::jsonb,
    position     INT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    version      BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX subflows_robot_id ON subflows (robot_id);
CREATE INDEX subflows_main_flow_id ON subflows (main_flow_id);

-- 变量没有代理 ID：流程表达式按名字引用，(robot_id, var_name) 就是业务主键。
-- 改名 = 删除 + 新建。
CREATE TABLE variables (
    robot_id                       VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    var_name                       TEXT NOT NULL,
    var_type                       TEXT NOT NULL,
    var_value_source               TEXT NOT NULL,
    var_constant_value             TEXT,
    var_associate_data             TEXT,
    obtain_value_expression_type   TEXT,
    obtain_value_expression        TEXT,
    connect_timeout_ms             BIGINT,
    read_timeout_ms                BIGINT,
    cache_enabled                  BOOLEAN,
    created_at                     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                     TIMESTAMPTZ NOT NULL DEFAULT now(),
    version                        BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (robot_id, var_name)
);

CREATE TABLE contexts (
    robot_id    VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    session_id  TEXT NOT NULL,
    data        JSONB NOT NULL,
    node_state  BYTEA, -- rkyv 节点状态，不塞进 JSON（D29）
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ,
    PRIMARY KEY (robot_id, session_id)
);
CREATE INDEX contexts_expires_at ON contexts (expires_at);

CREATE TABLE runtime_nodes (
    main_flow_id VARCHAR(32) NOT NULL REFERENCES mainflows (id) ON DELETE CASCADE,
    node_id      TEXT NOT NULL,
    data         BYTEA NOT NULL, -- rkyv 字节
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (main_flow_id, node_id)
);

-- ============================================================ 5.4 意图

CREATE TABLE intents (
    id         VARCHAR(32) PRIMARY KEY,
    robot_id   VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    position   INT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version    BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX intents_robot_id ON intents (robot_id);

CREATE TABLE intent_keywords (
    id        VARCHAR(32) PRIMARY KEY,
    intent_id VARCHAR(32) NOT NULL REFERENCES intents (id) ON DELETE CASCADE,
    position  INT NOT NULL,
    keyword   TEXT NOT NULL
);
CREATE INDEX intent_keywords_intent_id ON intent_keywords (intent_id);

CREATE TABLE intent_regexes (
    id        VARCHAR(32) PRIMARY KEY,
    intent_id VARCHAR(32) NOT NULL REFERENCES intents (id) ON DELETE CASCADE,
    position  INT NOT NULL,
    regex     TEXT NOT NULL
);
CREATE INDEX intent_regexes_intent_id ON intent_regexes (intent_id);

CREATE TABLE intent_phrases (
    id         VARCHAR(32) PRIMARY KEY,
    intent_id  VARCHAR(32) NOT NULL REFERENCES intents (id) ON DELETE CASCADE,
    position   INT NOT NULL,
    phrase     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX intent_phrases_intent_id ON intent_phrases (intent_id);

-- ============================================================ 5.5 外部 HTTP

CREATE TABLE external_http_apis (
    id               VARCHAR(32) PRIMARY KEY,
    robot_id         VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    description      TEXT,
    protocol         TEXT NOT NULL,
    method           TEXT NOT NULL,
    address          TEXT NOT NULL,
    post_content_type TEXT,
    request_body     TEXT,
    content_type     TEXT,
    user_agent       TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    version          BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX external_http_apis_robot_id ON external_http_apis (robot_id);

CREATE TABLE external_http_params (
    id           VARCHAR(32) PRIMARY KEY,
    api_id       VARCHAR(32) NOT NULL REFERENCES external_http_apis (id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('header', 'query', 'form')),
    position     INT NOT NULL,
    name         TEXT NOT NULL,
    value        TEXT NOT NULL,
    value_source TEXT NOT NULL -- Val / Var
);
CREATE INDEX external_http_params_api_id ON external_http_params (api_id);

-- ============================================================ 5.6 知识库

CREATE TABLE documents (
    id            VARCHAR(32) PRIMARY KEY,
    robot_id      VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    file_name     TEXT NOT NULL,
    file_size     BIGINT NOT NULL,
    mime_type     TEXT,
    original_path TEXT,
    content_hash  TEXT,
    doc_content   TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    version       BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX documents_robot_id ON documents (robot_id);

-- chunk_text 进 LLM prompt，不能小写化；chunk_text_norm 只用于关键词 LIKE（§15.4 S4.6）。
CREATE TABLE doc_chunks (
    id              VARCHAR(32) PRIMARY KEY,
    document_id     VARCHAR(32) NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    robot_id        VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    position        INT NOT NULL,
    chunk_text      TEXT NOT NULL,
    chunk_text_norm TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX doc_chunks_document_id ON doc_chunks (document_id);
CREATE INDEX doc_chunks_robot_id ON doc_chunks (robot_id);

CREATE TABLE qa (
    id         VARCHAR(32) PRIMARY KEY,
    robot_id   VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    answer     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version    BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX qa_robot_id ON qa (robot_id);

CREATE TABLE qa_questions (
    id         VARCHAR(32) PRIMARY KEY,
    qa_id      VARCHAR(32) NOT NULL REFERENCES qa (id) ON DELETE CASCADE,
    robot_id   VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    position   INT NOT NULL,
    question   TEXT NOT NULL,
    is_primary BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX qa_questions_qa_id ON qa_questions (qa_id);
CREATE INDEX qa_questions_robot_id ON qa_questions (robot_id);

-- ============================================================ 5.7 向量

-- ANN 索引 ID 与业务 ID 解耦（D7）：id 就是 VectorIndex 用的 u64。
CREATE TABLE vector_items (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    robot_id   VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    collection TEXT NOT NULL, -- phrase / qa / doc
    entity_id  VARCHAR(32) NOT NULL,
    generation BIGINT NOT NULL,
    vec        BYTEA NOT NULL,
    UNIQUE (collection, entity_id, generation)
);
CREATE INDEX vector_items_lookup ON vector_items (robot_id, collection, generation);

CREATE TABLE embedding_models (
    id          SMALLINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    provider    TEXT NOT NULL,
    model       TEXT NOT NULL,
    dimension   INT NOT NULL,
    normalized  BOOLEAN NOT NULL,
    fingerprint TEXT NOT NULL UNIQUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE vector_collection_state (
    robot_id          VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    collection        TEXT NOT NULL,
    active_generation BIGINT NOT NULL DEFAULT 0,
    active_model_id   SMALLINT REFERENCES embedding_models (id),
    pending_generation BIGINT,
    status            TEXT NOT NULL DEFAULT 'idle', -- idle / rebuilding / failed
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (robot_id, collection)
);

CREATE TABLE embedding_generations (
    robot_id   VARCHAR(32) NOT NULL REFERENCES robots (id) ON DELETE CASCADE,
    collection TEXT NOT NULL,
    generation BIGINT NOT NULL,
    model_id   SMALLINT NOT NULL REFERENCES embedding_models (id),
    status     TEXT NOT NULL, -- pending / running / done / failed
    started_at  TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    PRIMARY KEY (robot_id, collection, generation)
);

-- ============================================================ 5.8 身份与审计（v1 裁减版，D42/D43）

CREATE TABLE users (
    id                   VARCHAR(32) PRIMARY KEY,
    username             TEXT NOT NULL UNIQUE,
    display_name         TEXT,
    email                TEXT,
    password_hash        TEXT NOT NULL, -- argon2id
    status               TEXT NOT NULL DEFAULT 'active',
    must_change_password BOOLEAN NOT NULL DEFAULT FALSE,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at        TIMESTAMPTZ
);

CREATE TABLE sessions (
    id           VARCHAR(32) PRIMARY KEY,
    user_id      VARCHAR(32) NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   BYTEA NOT NULL UNIQUE, -- 只存哈希，不存原始 token
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,
    ip           TEXT,
    user_agent   TEXT
);
CREATE INDEX sessions_user_id ON sessions (user_id);

-- append-only，无 hash 链（D43）
CREATE TABLE audit_log (
    entry_seq      BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    id             VARCHAR(32) NOT NULL,
    ts             TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor_user_id  VARCHAR(32),
    actor_name     TEXT NOT NULL,
    action         TEXT NOT NULL,
    resource_type  TEXT,
    resource_id    TEXT,
    result         TEXT NOT NULL,
    ip             TEXT,
    user_agent     TEXT,
    detail         JSONB
);
CREATE INDEX audit_log_ts ON audit_log (ts);

-- 检索审计：高 volume，不套链，不分区，按 ts 范围 DELETE 清理（D43）
CREATE TABLE audit_access_log (
    id           BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ts           TIMESTAMPTZ NOT NULL DEFAULT now(),
    actor_user_id VARCHAR(32),
    robot_id     VARCHAR(32),
    collection   TEXT,
    query_text   TEXT,
    result_count INT,
    latency_ms   INT,
    ip           TEXT
);
CREATE INDEX audit_access_log_ts ON audit_access_log (ts);
