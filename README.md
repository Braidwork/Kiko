[简体中文](./README_zh-CN.md) | English

Welcome to give this project a star on [GitHub](https://github.com/Braidwork/Kiko) ✨**Star**🎇!

# Kiko —— A Deployable Conversation Bot Platform

Kiko is a **deployable conversation bot platform**. Orchestrate conversations with drag-and-drop flow design, understand users through intents/keywords/regex, perform retrieval-based Q&A with knowledge bases, and integrate with local models or any `OpenAI`-compatible interface. It is delivered as a single executable file, with business data stored in your own `PostgreSQL` — ideal for scenarios where "data cannot leave the premises and you don't want to maintain a bunch of middleware."

It is the **enterprise private deployment** fork of `dialogflow-rs` (Apache-2.0). The biggest difference from upstream is **storage**: knowledge bases, intents, flows, sessions, and vectors all go into the customer's own `PostgreSQL`, while `data/` only holds model weights, uploaded original files, configuration, and the master key. The benefit is operability: backup, restore, monitoring, auditing, and capacity planning all leverage the customer's existing PG infrastructure, rather than having binaries scattered in `data/`.

<img src="https://img.shields.io/badge/version-0.1.0-blue" /> <img src="https://img.shields.io/badge/MSRV-1.94-green" /> <img src="https://img.shields.io/badge/PostgreSQL-14%2B-336791" /> <img src="https://img.shields.io/badge/license-Apache--2.0-lightgrey" />

![All in ONE](./doc/assets/allinone.png)

![](./doc/assets/screenshots/flow-editor-zh.png)

## Differences Between Kiko and dialogflow-rs

| Dimension | dialogflow-rs (upstream) | **Kiko (this repo)** |
| --- | --- | --- |
| Positioning | Out-of-the-box single machine | Enterprise private deployment, targeting delivery scenarios where "the customer provides the database" |
| Business Data | Embedded redb (later turso) | **External PostgreSQL**, fixed schema `kiko`, does not occupy `public` |
| Knowledge Base / Vectors | Same embedded storage as metadata | `documents` / `doc_chunks` / `qa` + `vector_items` etc. all go into PG |
| Installation Dependencies | Zero dependencies | Requires PostgreSQL (development uses 18, 14+ recommended) |
| Schema Management | None (data files move with the program) | `sqlx::migrate!` embedded migrations, auto-migrate on startup (`auto_migrate`) |
| Concurrency Correctness | Single write queue, read-modify-write has a window for lost updates | Transactions + row locks + **optimistic locking on all editable entities** (`version`), conflicts explicitly error |
| Sensitive Configuration | Plaintext locally | `secrets` table + `enc:`/`env:`/`file:` reference strings, AES-GCM encryption, master key can be external |
| Switching Embedding Models | Change config to take effect (may mix old and new vectors) | **Active model separation**: queries/incremental writes follow the "model with existing index", switching only happens on rebuild |
| Vector Index | turso internal search | PG as source of truth + in-process brute-force index (memory budget, f16 precision reduction, dirty marking, background reconciliation) |
| Restart and Resume | Depends on redb/turso file consistency | Runtime also in PG; rebuild scan is idempotent and resumable (without jobs table) |
| Configuration | Environment variables / built-in | `data/kiko.toml` + `KIKO_*` overrides (priority: env vars > file > defaults) |
| API Contract | Includes internal row IDs | External-facing only uses scru128 primary keys, internal row IDs are not exposed |

## ✨ Highlights

* 🗄️ **Database as Source of Truth**: No embedded databases, no turso/redb file locks. Backup = PG backup.
  + `data/uploads/` (plus `data/master.key`), restore, master-slave, monitoring all handed to the customer's existing PG infrastructure.
* 📦 **Still a Single Binary**: Rust backend + Vue3 frontend compiled into one executable; `data/` only holds model weights, uploaded original files, `kiko.toml`, and master key — no business data left behind.
* 🔎 **Auditable Knowledge Base**: DOCX upload → chunking (original `chunk_text` + normalized `chunk_text_norm`) → vectors, original text is searchable and traceable; keyword recall and vector recall fused via RRF.
* 🧠 **Operable Vector Index**: `[vector]` configures memory budget (default 512 MB), over budget first reduces to f16, then evicts entire collections by LRU; background reconciles by row count with PG, marks `dirty` on drift; rebuilds are idempotent and resumable by generation.
* 🔁 **Safe Model Switching**: Queries and incremental writes use the model pointed to by `active_model_id`, only switching globally after rebuild completes; no silent errors of "new query vectors vs old database vectors."
* 🔐 **Sensitive Configs Not in Plaintext**: API keys / passwords stored as reference strings, master key can be externalized via `KIKO_MASTER_KEY`; not written to logs, not echoed.
* 🧩 **Concurrency Safety**: Documents, Q&A, canvases, main flows, bots, variables, external HTTP edits all carry `version` validation; missing version rejected outright, stale versions return conflict, no silent overwrites.
* ⚙️ **Configuration Transparency**: `data/kiko.toml` template comes with full comments, every `KIKO_*` override has a one-to-one correspondence.
* 🧪 **Testable**: PG integration tests use `tmp_<uuid>` schema, dropped after execution, no pollution of shared databases.
* 🧬 **Inherits Upstream**: Flow engine, 9 node types, intents/keywords/regex/phrases, streaming responses, OpenAI-compatible interfaces and local `HuggingFace` models all preserved, frontend is the same Vue3 UI.

## Key Features (Inherited from Upstream)

* 🐱‍🏍 **AI-Driven**: Local `Huggingface` models (Llama, Phi-3, Gemma, Multilingual E5, MiniLM L6v2, NomicEmbedTextV1_5, etc.) and any OpenAI-compatible interface (OpenAI, DeepSeek, Zhipu GLM, Tongyi Qianwen, Kimi, SiliconFlow, Groq, OpenRouter, Ollama, vLLM, LM Studio, or custom gateway), usable for flow chat, answer generation, and intent recognition.
* 😀 **Visual Orchestration**: Drag nodes to build bots, supporting conditional branching, information collection, jumps, external HTTP, email, knowledge base answers, etc.
* 🚀 **Rust + Vue**: Single-process, multi-threaded runtime, no Node/Python runtime dependencies.
* 🔐 **Data Stays Within Boundary**: Business data in customer PG, models and uploaded files in customer `data/`; only leaves the network when external model APIs are explicitly configured.

## Feature Nodes

| Node | Name |
| --- | --- |
| ![DialogNode](./doc/assets/screenshots/dialogNode.png) | Dialog Answer Node |
| ![LLM chat node](./doc/assets/screenshots/llmChatNode.png) | LLM Chat Node |
| ![](./doc/assets/screenshots/knowledgeBaseAnswerNode.png) | Knowledge Base Answer Node |
| ![](./doc/assets/screenshots/conditionNode.png) | Condition Node |
| ![](./doc/assets/screenshots/gotoNode.png) | Jump Node |
| ![](./doc/assets/screenshots/collectNode.png) | Information Collection Node |
| ![](./doc/assets/screenshots/externalApiNode.png) | External API Request Node |
| ![](./doc/assets/screenshots/sendEmailNode.png) | Email Send Node |
| ![](./doc/assets/screenshots/theEndNode.png) | Flow End Node |

## Streaming Responses

`POST /flow/answer` returns a JSON document by default; when the request body includes `"stream": true`, it switches to frame-by-frame push, and the response is `application/x-ndjson`: each line is a JSON object, `{"contentSeq": 0, "content": "..."}` is an answer fragment, and the last line is always the final frame (`contentSeq: null`), whose `content` is the complete `{status, data, err}` response. Full protocol details in [`doc/streaming.md`](doc/streaming.md).

```bash
curl -N -H 'Content-Type: application/json' \
  -d '{"robotId":"...","mainFlowId":"...","userInput":"Hello","stream":true}' \
  http://127.0.0.1:12715/flow/answer
```

## Getting Started

### 1. Prepare PostgreSQL

Kiko requires a writable PostgreSQL database (development uses 18, 14+ recommended). No Redis, ElasticSearch, or vector database needed.

```sql
CREATE DATABASE kiko;
CREATE USER kiko WITH PASSWORD 'change-me';
GRANT ALL PRIVILEGES ON DATABASE kiko TO kiko;
```

### 2. Write Configuration

Defaults to reading `data/kiko.toml` (use `KIKO_CONFIG` to specify another path). **On first startup, if the file doesn't exist, the program generates a commented template and exits** — fill in the connection string and restart:

```toml
[database]
# You can also set only KIKO_DATABASE_URL (higher priority than this file), but the config file itself must exist
url = "postgres://kiko:change-me@127.0.0.1:5432/kiko"
# Auto-execute embedded migrations on startup (`--migrate-only` planned for S5)
auto_migrate = true

[security]
# Leave empty to auto-generate data/master.key on first startup (Unix permissions 0600).
# Can also be injected via KIKO_MASTER_KEY.
master_key = ""

[vector]
# Total memory budget (MB) for all loaded indexes, 0 = use default 512
index_budget_mb = 512
# Background reconciliation interval (seconds), 0 = disabled
reconcile_secs = 60
```

### 3. Run

```bash
kiko                          # default 127.0.0.1:12715
kiko -ip 0.0.0.0 -port 8888   # specify listen address/port
```

First startup will: connect to PG → run migrations → generate `data/master.key` (if not configured) → start HTTP service. Open `http://127.0.0.1:12715/` in your browser.

> The release process for precompiled packages and Docker images is part of S5 and has not been solidified yet. Currently building from source: first produce frontend assets in `frontend/`, place them in `src/resources/assets/`, then `cargo build --release` (this pipeline is an S5 packaging item, not yet automated).

## Configuration Options and Overrides

Priority: **Environment Variables > `data/kiko.toml` > Built-in Defaults**.

| Config | File Key | Environment Variable | Description |
| --- | --- | --- | --- |
| Config File Path | — | `KIKO_CONFIG` | Default `data/kiko.toml` |
| Database Connection String | `[database] url` | `KIKO_DATABASE_URL` | Required (or provided by env) |
| Auto Migrate | `[database] auto_migrate` | — | Default `true` |
| Master Key | `[security] master_key` | `KIKO_MASTER_KEY` | Leave empty to auto-generate `data/master.key` |
| Vector Index Budget | `[vector] index_budget_mb` | `KIKO_VECTOR_INDEX_BUDGET_MB` | MB, 0 = 512 |
| Background Reconcile Interval | `[vector] reconcile_secs` | `KIKO_VECTOR_RECONCILE_SECS` | Seconds, 0 = disabled |

## Development

```bash
cargo check                     # fast
cargo test                      # unit tests
KIKO_TEST_DATABASE_URL=postgres://postgres:password@127.0.0.1:5432/kiko_dev cargo test
cargo clippy --all-targets
cargo fmt --check
```

Frontend source is in `frontend/`:

```bash
cd frontend && pnpm install && pnpm dev   # http://localhost:5173, API points to 127.0.0.1:12715
```

> When only changing frontend source code, do not manually edit `src/resources/assets/` or `src/web/asset.rs` — they are build artifacts. Frontend assets embedded into the binary is an S5 packaging item, not yet automated.

## Documentation

* [`doc/streaming.md`](doc/streaming.md) — `/flow/answer` streaming protocol.
* [`README.md`](README.md) — Upstream English README (not yet synced with Kiko, refer to this page).

## License

Apache-2.0, see [`LICENSE`](LICENSE).


## We also provide Community and Government & Enterprise Editions

| Feature | Community Edition | Enterprise Edition | Government & Enterprise Edition |
|---|---|---|---|
| Visual node editor | Supported | Supported | Supported |
| Dialogue nodes | Supported | Supported | Supported |
| LLM chat nodes | Supported | Supported | Supported |
| Knowledge base answer nodes | Supported | Supported | Supported |
| Condition nodes | Supported | Supported | Supported |
| Collection nodes | Supported | Supported | Supported |
| Jump nodes | Supported | Supported | Supported |
| External HTTP request nodes | Supported | Supported | Supported |
| Email sending nodes | Supported | Supported | Supported |
| End nodes | Supported | Supported | Supported |
| Intent recognition | Supported | Supported | Supported |
| Knowledge base | Supported | Supported | Supported |
| Variable support | Supported | Supported | Supported |
| Create dialogue flows via API | Not supported | Supported | Supported |
| Data export/import | Not supported | Supported | Supported |
| SQL Server | Not supported | WIP | Supported |
| SQLite | Not supported | WIP | Supported |
| PostgreSQL | Not supported | Supported | Supported |
| MySQL | Not supported | WIP | Supported |
| Audit | Not supported | Supported | Supported |
| Xinchuang / localized ecosystem | Not supported | Not supported | Supported |
| License | Apache License 2.0 | Apache License 2.0 | Apache License 2.0 |
| Technical support | GitHub issues | Email | Email |
| Project link | https://github.com/dialogflowai/dialogflow | https://github.com/Braidwork/Kiko | Email: dialogflow@yeah.net |
