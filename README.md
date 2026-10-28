# Kiko — Self-hosted Chatbot Platform

Welcome to give this project a star on [GitHub](https://github.com/Braidwork/Kiko) ✨**Star**🎇!

Kiko is a **self-hosted chatbot platform**. Build conversational flows with a visual drag-and-drop builder, understand users via intents/keywords/regex, perform RAG-based knowledge base Q&A, and integrate with local models or any `OpenAI`-compatible API. It ships as a single binary with your business data stored in your own `PostgreSQL` — ideal for scenarios where "data cannot leave premise and you don't want to maintain a bunch of middleware."

It is the **enterprise private deployment** fork of `dialogflow-rs` (Apache-2.0). The biggest difference from upstream is **storage**: knowledge base, intents, flows, sessions, and vectors all go into the customer's own `PostgreSQL`; `data/` only holds model weights, uploaded original files, configuration, and the master key. The key benefit is operational manageability: backup, restore, monitoring, audit, and capacity planning all leverage the customer's existing PG infrastructure rather than scattered binary files in `data/`.

[![title=,alt=]()](<image-ocr:a60d276440336bf2ac6ef7b389ae2eed>) [![title=,alt=]()](<image-ocr:b8cfdc4bb93514c5ba0b9e476f5824f3>) [![title=,alt=]()](<image-ocr:a3a88c884fcb38b0179808e90ad5fdf8>) [![title=,alt=]()](<image-ocr:b2869bfc6221900650f57cd10f304f58>)

[![title=,alt=All in ONE]()](<image-ocr:1d6d71eb91050359bbf28bcd9b07aed1>)

[![title=,alt=]()](<image-ocr:3ae30537596a7dca2ea1bb2eb2294a13>)

## Differences from dialogflow-rs

| Dimension | dialogflow-rs (upstream) | **Kiko (this repo)** |
| --- | --- | --- |
| Positioning | Out-of-the-box single machine | Enterprise private deployment, targeting "customer provides database" delivery scenarios |
| Business Data | Embedded redb (later turso) | **External PostgreSQL**, fixed schema `kiko`, does not use `public` |
| Knowledge Base / Vectors | Same embedded storage as metadata | `documents` / `doc_chunks` / `qa` + `vector_items` etc. all go into PG |
| Installation Dependencies | Zero | Requires PostgreSQL (18 for dev verification, 14+ recommended) |
| Schema Management | None (data files travel with the program) | `sqlx::migrate!` embedded migrations, auto-migrate on startup (`auto_migrate`) |
| Concurrency Correctness | Single write queue, read-modify-write has stale update window | Transactions + row locks + **optimistic locking on all editable entities** (`version`), explicit conflict errors |
| Sensitive Config | Local plaintext | `secrets` table + `enc:`/`env:`/`file:` reference strings, AES-GCM encryption, master key can be externalized |
| Switching Embedding Models | Change config to take effect (may mix old/new vectors) | **Active model separation**: queries/incremental writes follow the "indexed model", only full rebuild switches |
| Vector Index | turso internal search | PG as source of truth + in-process brute-force index (memory budget, f16 quantization, dirty marking, background reconciliation) |
| Restart & Resume | Depends on redb/turso file consistency | Runtime also in PG; rebuild scans are idempotent, interruptible resume (no jobs table) |
| Configuration | Environment variables / built-in | `data/kiko.toml` + `KIKO_*` overrides (priority: env vars > file > defaults) |
| API Contract | Includes internal row IDs | External-facing only uses scru128 primary keys, internal row IDs not exposed |

## Highlights

* 🗄️ **Database as source of truth**: No embedded DB, no turso/redb file locks. Backup = PG backup.
    * `data/uploads/` (plus `data/master.key`), restore, master-replica, monitoring all handed off to the customer's existing PG infrastructure.
* 📦 **Still a single binary**: Rust backend + Vue3 frontend compiled into one executable; `data/` only holds model weights, uploaded original files, `kiko.toml`, and master key — business data all stays local.
* 🔎 **Auditable knowledge base**: DOCX upload → chunking (original `chunk_text` + normalized `chunk_text_norm`) → vectors, original text queryable and traceable; keyword recall and vector recall fused via RRF.
* 🧠 **Operable vector index**: `[vector]` configures memory budget (default 512 MB), over budget first quantize to f16, then evict entire collections by LRU; background reconcile by row count against PG, mark `dirty` on drift; rebuild resumes idempotently by generation.
* 🔁 **Safe model switching**: Queries and incremental writes use the model pointed to by `active_model_id`, only switch overall after rebuild completes; no silent "new query vectors vs old store vectors" errors.
* 🔐 **Sensitive config not in plaintext**: API keys / passwords stored as reference strings, master key externalizable via `KIKO_MASTER_KEY`; never written to logs, never echoed.
* 🧩 **Concurrency safe**: Edits to documents, Q&A, canvas, main flow, bots, variables, external HTTP all carry `version` checks; missing version rejected outright, stale versions return conflict, no more silent overwrites of others' changes.
* ⚙️ **Transparent configuration**: `data/kiko.toml` template with full comments, every `KIKO_*` override has a one-to-one correspondence.
* 🧪 **Testable**: PG integration tests use `tmp_<uuid>` schema, dropped after each run, no pollution of shared databases.
* 🧬 **Inherits upstream**: Flow engine, 9 node types, intents/keywords/regex/phrases, streaming responses, OpenAI-compatible API and local `HuggingFace` models all preserved, frontend is the same Vue3 UI.

## Key Features (Inherited from Upstream)

* 🐱‍🏍 **AI-driven**: Local `Huggingface` models (Llama, Phi-3, Gemma, Multilingual E5, MiniLM L6v2, NomicEmbedTextV1_5 etc.) and any OpenAI-compatible API (OpenAI, DeepSeek, Zhipu GLM, Tongyi Qianwen, Kimi, SiliconFlow, Groq, OpenRouter, Ollama, vLLM, LM Studio or self-hosted gateway), usable for flow chat, answer generation, and intent recognition.
* 😀 **Visual orchestration**: Build bots by dragging nodes, supporting conditional branches, information collection, jumps, external HTTP, email, knowledge base answers, etc.
* 🚀 **Rust + Vue**: Single process, multi-threaded runtime, no Node/Python runtime dependencies.
* 🔐 **Data stays in-domain**: Business data in customer PG, models and uploaded files in customer `data/`; only exits network when external model API is explicitly configured.

## Functional Nodes

| Node | Name |
| --- | --- |
|  | Conversation Answer Node |
|  | LLM Chat Node |
|  | Knowledge Base Answer Node |
|  | Condition Node |
|  | Jump Node |
|  | Information Collection Node |
|  | External API Request Node |
|  | Email Send Node |
|  | Flow End Node |

## Streaming Responses

`POST /flow/answer` returns a JSON document by default; when the request body includes `"stream": true`, it switches to frame-by-frame push with `application/x-ndjson` response — one JSON per line, `{"contentSeq": 0, "content": "..."}` is an answer fragment, and the last line is always the terminal frame (`contentSeq: null`) whose `content` is the complete `{status, data, err}` response. Full protocol in [`doc/streaming.md`](doc/streaming.md).

```shell
curl -N -H 'Content-Type: application/json' \
  -d '{"robotId":"...","mainFlowId":"...","userInput":"你好","stream":true}' \
  http://127.0.0.1:12715/flow/answer
```

## Getting Started

### 1. Prepare PostgreSQL

Kiko requires a writable PostgreSQL database (18 for dev verification, 14+ recommended). No Redis, ElasticSearch, or vector database needed.

```sql
CREATE DATABASE kiko;
CREATE USER kiko WITH PASSWORD 'change-me';
GRANT ALL PRIVILEGES ON DATABASE kiko TO kiko;
```

### 2. Write Configuration

Defaults to reading `data/kiko.toml` (use `KIKO_CONFIG` to specify another path). **On first startup, if the file doesn't exist, the program generates a commented template and exits** — fill in the connection string and restart:

```toml
[database]
# You can also set KIKO_DATABASE_URL (higher priority than this file), but the config file itself must exist
url = "postgres://kiko:change-me@127.0.0.1:5432/kiko"
# Auto-run embedded migrations on startup (--migrate-only planned for S5)
auto_migrate = true

[security]
# Leave blank to auto-generate data/master.key on first startup (Unix permissions 0600).
# Can also be injected via KIKO_MASTER_KEY.
master_key = ""

[vector]
# Total memory budget (MB) for all loaded indices, 0 = use default 512
index_budget_mb = 512
# Background reconciliation interval (seconds), 0 = disabled
reconcile_secs = 60
```

### 3. Run

```shell
kiko                          # default 127.0.0.1:12715
kiko -ip 0.0.0.0 -port 8888   # specify listen address/port
```

First startup will: connect to PG → run migrations → generate `data/master.key` (if not configured) → start HTTP service. Open `http://127.0.0.1:12715/` in your browser.

> Pre-compiled packages and Docker image release flows are S5 and not yet solidified. Currently build from source: first produce frontend assets in `frontend/`, place them in `src/resources/assets/`, then `cargo build --release` (this pipeline is an S5 packaging item, not yet automated).

## Configuration Options and Overrides

Priority: **Environment Variables > `data/kiko.toml` > Built-in Defaults**.

| Config | File Key | Environment Variable | Description |
| --- | --- | --- | --- |
| Config file path | — | `KIKO_CONFIG` | Default `data/kiko.toml` |
| Database connection string | `[database] url` | `KIKO_DATABASE_URL` | Required (or provided by env) |
| Auto migrate | `[database] auto_migrate` | — | Default `true` |
| Master key | `[security] master_key` | `KIKO_MASTER_KEY` | Auto-generate `data/master.key` if blank |
| Vector index budget | `[vector] index_budget_mb` | `KIKO_VECTOR_INDEX_BUDGET_MB` | MB, 0 = 512 |
| Background reconcile interval | `[vector] reconcile_secs` | `KIKO_VECTOR_RECONCILE_SECS` | Seconds, 0 = disabled |

## Development

```shell
cargo check                     # fast
cargo test                      # unit tests
KIKO_TEST_DATABASE_URL=postgres://postgres:password@127.0.0.1:5432/kiko_dev cargo test
cargo clippy --all-targets
cargo fmt --check
```

Frontend source in `frontend/`:

```shell
cd frontend && pnpm install && pnpm dev   # http://localhost:5173, API points to 127.0.0.1:12715
```

> When only changing frontend source, don't manually edit `src/resources/assets/` or `src/web/asset.rs` — they are build artifacts. Frontend assets embedded into the binary is an S5 packaging item, not yet automated.

## Documentation

* [`doc/streaming.md`](doc/streaming.md) — `/flow/answer` streaming protocol.
* [`README.md`](README.md) — Upstream English README (not yet synced with Kiko, refer to this page as source of truth).

## License

Apache-2.0, see [`LICENSE`](LICENSE).
