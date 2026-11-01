# Kiko 单二进制打包（B-51）

> 目标：交付一个 `kiko(.exe)`：Rust 后端 + 内嵌前端产物 + 内嵌 migration；
> 目标机只需要外部 PostgreSQL，不需要 Rust/Node。
>
> **本文件是流程与脚本说明。** 2026-10-31 经用户授权后已执行一次完整出包：
> `pnpm build` → 备份并替换 `src/resources/assets/` → `cargo build --release`；
> 产物为 `target/release/kiko.exe`（argon2 0.6 重构建后 20,974,592 字节）。本文件同时保留可复现步骤。
> 没有 commit/push；生成的 `src/resources/assets/`、`src/web/asset.{rs,txt}` 按
> 仓库约定不提交。

---

## 1. 产物链路

```
frontend/src/**  ──pnpm build──▶  frontend/dist/**
                                      │ 拷贝
                                      ▼
                           src/resources/assets/**
                                      │ build.rs：逐文件 gzip + 生成映射
                                      ▼
                     src/web/asset.{rs,txt}（generated，gitignore）
                                      │ include_bytes!
                                      ▼
                              kiko(.exe) 内嵌
```

- `src/resources/assets/`、`src/web/asset.rs`、`src/web/asset.txt` 都是生成物，
  **不手改、不提交**。
- migration 由 `sqlx::migrate!("./migrations/pg")` 编译期内嵌；不需要随包发 SQL。
- 最终只需要：`kiko` 二进制 + `data/`（配置、主密钥、上传文件、模型）+ 外部 PG。

---

## 2. 前置条件

| 工具 | 版本/说明 |
| --- | --- |
| Rust | MSRV 1.94；`cargo` 可用 |
| Node.js + pnpm | 前端构建；`frontend/pnpm-lock.yaml` 为准 |
| 目标平台 | Windows 产物在 Windows 构建；Linux/musl 需在对应环境构建 |
| PostgreSQL | 只用于启动验证，不参与打包 |

构建机可以联网装依赖；**运行目标机默认不联网**（D38）。

---

## 3. 打包步骤

### 3.1 构建前端

```powershell
cd frontend
pnpm install --frozen-lockfile
pnpm build
```

`pnpm build` 会先跑 `compress`（SDK 压缩）再 `vite build`，产物在 `frontend/dist/`。
不要用 `frontend/b.bat`：它会 `cls`、整目录删除并覆盖生成物，不适合在代理/CI 内直接跑。

### 3.2 替换内嵌资源

先备份旧生成物，再把 `dist/*` 拷到 `src/resources/assets/`：

```powershell
$root = (Get-Location).Path
$assets = Join-Path $root 'src/resources/assets'
$backup = Join-Path $root ('data/assets-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $backup -Force | Out-Null
Copy-Item (Join-Path $assets '*') $backup -Recurse -Force
Get-ChildItem -LiteralPath $assets -Force | Remove-Item -Recurse -Force
Copy-Item (Join-Path $root 'frontend/dist/*') $assets -Recurse -Force
```

- 带内容哈希的旧文件如果不清掉，`build.rs` 会把它们也编进二进制（体积变大但不报错）；
  因此这里在**备份之后**才清空目录。
- `build.rs` 会在 `src/resources/assets/` 里生成 `.gz`；这些是 ignored 生成物。

### 3.3 构建 Rust release

```powershell
cargo build --release
```

产物：`target/release/kiko.exe`（Windows）或 `target/release/kiko`（Unix）。
**不要为了日常验证跑 release 构建**；只有确实要出包时才执行。

### 3.4 一键脚本（可选）

`scripts/package.ps1` 按上面步骤执行，并保留旧资源备份：

```powershell
# 完整出包（会跑前端构建 + release 构建）
.\scripts\package.ps1

# 前端已构建好，只重新嵌入并编译
.\scripts\package.ps1 -SkipFrontend

# 只替换资源，不跑 release（例如先做开发验证）
.\scripts\package.ps1 -SkipFrontend -SkipReleaseBuild
```

脚本不会 commit/push，也不会改 `frontend/b.bat`。

---

## 4. 验证

### 4.1 资源是否真的内嵌

1. `src/web/asset.txt` 里应出现新的 `index-*.js` / `index-*.css` / `index.html`；
2. `cargo test --lib public_routes_and_static_assets_are_not_401` 应返回 200 且带
   `x-content-type-options`；
3. 启动二进制后：

```bash
curl -I http://127.0.0.1:12715/
# 200，content-type: text/html，content-encoding: gzip
curl -s http://127.0.0.1:12715/version.json
# "x.y.z"
```

4. 浏览器打开首页，确认 JS/CSS 没有 404、登录页能加载。

### 4.2 migration 是否内嵌

```bash
kiko --check-schema
# 输出 database_schema_version / program_schema_version
```

不需要把 `migrations/` 拷到目标机。

### 4.3 目标机依赖检查

- 只有 `kiko` + `data/` + PG；没有 `node_modules`、`cargo`、`migrations/` 也能启动；
- `ldd`（Linux）应只依赖系统库；musl 构建见 Cargo 配置；
- 运行前设置 `KIKO_DATABASE_URL` 或 `data/kiko.toml`。

---

## 4.4 开发模式（`pnpm dev`）

开发服务器默认在 `http://127.0.0.1:5173`，通过 Vite 代理访问后端：

```
浏览器 /api/auth/login  →  Vite proxy  →  http://127.0.0.1:12715/auth/login
```

- `.env.development` 的 `VITE_REQ_BACKEND_PREFIX=/api/`；`vite.config.js` 的
  `server.proxy` 把 `/api/*` 重写为 `/*`。浏览器始终同源，HttpOnly cookie 不需要
  CORS，也不会触发 `OPTIONS` 预检。
- 登录页地址是 **`http://127.0.0.1:5173/#/login`**（hash 路由），不是
  `/auth/login`。直接打开 `/auth/login` 是 GET 一个只接受 POST 的 API，返回 405
  是预期行为；要访问页面请用 `#/login`。
- 改完 `.env.development` 或 `vite.config.js` 后必须重启 `pnpm dev`（Vite 不会热加载
  这些配置）。
- 如果后端不在 12715，改 `vite.config.js` 里的 proxy `target`，或者用
  `[server] cors_origins` 配跨源（不推荐，会重新引入预检）。
- `pnpm dev` 不是交付形态；交付仍然是 §3 的单二进制。

---

## 5. 发布物清单

| 文件 | 必需 | 说明 |
| --- | --- | --- |
| `kiko(.exe)` | 是 | 单二进制 |
| `data/kiko.toml` | 是 | 首次可让程序生成模板；DSN 必填 |
| `data/master.key` | 是 | 已有 `enc:` 数据时必须与备份一致 |
| `data/uploads/` | 按需 | 原始文件；与数据库同时间点 |
| `data/models/` | 按需 | 离线模型；不随二进制 |
| `doc/upgrade.md` / `doc/storage.md` | 交付文档 | 升级、备份、容量 |

不要打包：`target/`、`frontend/node_modules/`、`frontend/dist/`、`.git/`、
`data/initial_admin_password.txt`（首次启动生成，改密后删除）。

---

## 6. 已知边界

- 已执行：前端生产构建、资源替换、release 编译、release 二进制 `/healthz` 与
  静态资源冒烟；浏览器 E2E 见 `doc/upgrade.md` 与 §18 第十四片。
- 未执行：B-55 两版本升级演练（需要旧二进制和独立数据库）、B-54 断网下载回归。
- 跨平台产物需在对应平台构建；Windows 产物不能直接当 Linux 产物用。
- 前端资源带内容哈希；替换后必须重新编译 Rust（`build.rs` 会重新生成
  `asset.{rs,txt}`），不能只换 `dist/`。
- 如果客户要求“无 Node、无 Rust 的目标机”，交付的就是 §5 的二进制 + `data/`；
  构建机上的工具链不进入交付物。
