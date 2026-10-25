//! 启动配置：连库之前就必须可用的那一小撮（D26 / D33 / D34）。
//!
//! 分层（D26）：
//! - **本模块**只负责「文件 + 环境变量」这一层，也就是 DSN、迁移开关这类必须在连库前
//!   就知道的东西；
//! - 业务设置（模型供应商、SMTP、ASR/TTS…）后面搬进 PG 的 `global_settings` /
//!   `robot_settings`，不在这里。
//!
//! 优先级：**环境变量 > 配置文件 > 内置默认**（D33）。没有内置 DSN —— 猜一个连不上
//! 的库只会让现场更难排查，所以缺 DSN 一律 fail-fast。

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 配置文件路径的环境变量覆盖（D34）。
pub(crate) const CONFIG_PATH_ENV: &str = "KIKO_CONFIG";
/// DSN 的环境变量覆盖，优先于配置文件里的 `[database] url`。
pub(crate) const DATABASE_URL_ENV: &str = "KIKO_DATABASE_URL";
/// 主密钥的环境变量覆盖（D20），优先于 `[security] master_key`。
pub(crate) const MASTER_KEY_ENV: &str = "KIKO_MASTER_KEY";
/// 进程内向量索引内存预算的环境变量覆盖，优先于 `[vector] index_budget_mb`。
pub(crate) const VECTOR_BUDGET_ENV: &str = "KIKO_VECTOR_INDEX_BUDGET_MB";
/// 向量索引后台对账周期的环境变量覆盖，优先于 `[vector] reconcile_secs`。
pub(crate) const VECTOR_RECONCILE_ENV: &str = "KIKO_VECTOR_RECONCILE_SECS";
/// 默认配置文件。仓库根的 `config.toml` 是 rustc 构建配置，两者不许重名（D34）。
pub(crate) const DEFAULT_CONFIG_PATH: &str = "data/kiko.toml";

/// 向量索引默认预算（MB）：10 万条 × 1024 维 f32 ≈ 400 MB，f16 ≈ 204 MB。
const DEFAULT_VECTOR_BUDGET_MB: u64 = 512;
/// 后台对账默认周期（秒）。
const DEFAULT_VECTOR_RECONCILE_SECS: u64 = 60;

const TEMPLATE: &str = r#"# Kiko 应用配置（D34）。
#
# 仓库根的 config.toml 是 rustc 构建配置，不是这个文件。
# 优先级：环境变量 > 本文件 > 内置默认。
#
# 首次运行缺少本文件时会生成这份模板并以非 0 退出（D33）：
# 填好 [database] url 后重启即可，没有 setup 向导。
#
# 环境变量（都优先于本文件里的同名项；不想用环境变量就删掉下面相关的注释即可）：
#   KIKO_CONFIG                    换一份配置文件（本变量本身只能来自环境变量）
#   KIKO_DATABASE_URL              [database] url
#   KIKO_MASTER_KEY                [security] master_key
#   KIKO_VECTOR_INDEX_BUDGET_MB    [vector] index_budget_mb
#   KIKO_VECTOR_RECONCILE_SECS     [vector] reconcile_secs

[database]
# PostgreSQL 连接串，例如 postgres://kiko:password@127.0.0.1:5432/kiko_dev
# schema 固定为 kiko（D37），应用会自动创建，不需要写进连接串。
url = ""

# 启动时自动执行 migration；由 DBA 统一跑 DDL 的部署改成 false（D45）。
auto_migrate = true

[security]
# 主密钥（base64 的 32 字节），用于加密 settings 里的敏感字段（D20/D36）。
# 优先级：KIKO_MASTER_KEY 环境变量 > 这里的值 > 自动生成的 data/master.key。
# 留空即可：首次启动会生成 data/master.key（Unix 下 0600）。
master_key = ""

# 进程内向量索引（D35 / B-27 / B-28）。索引是缓存，PG 是事实来源。
[vector]
# 所有 (robot, collection) 常驻索引的总内存预算，单位 MB；0 = 使用默认 512。
# 超过预算时先降成 f16（内存约一半），再按"最久未用"淘汰整个集合。
# 参考：10 万条 × 1024 维 f32 ≈ 400 MB，f16 ≈ 204 MB；100 万条 × 1024 维 f32 ≈ 4 GB。
index_budget_mb = 512

# 后台对账周期（秒）：按行数比对已载入索引与 PG，漂移就作废缓存并标 dirty。
# 0 = 关闭对账（单实例且完全不改库外数据时可以关）。
reconcile_secs = 60
"#;

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) database_url: String,
    pub(crate) auto_migrate: bool,
    /// 配置文件里的 `[security] master_key`；空/缺失时由 `security` 自动生成。
    pub(crate) master_key: Option<String>,
    /// 向量索引内存预算（MB，已合并环境变量覆盖，保证 > 0）。
    pub(crate) vector_index_budget_mb: u64,
    /// 向量索引后台对账周期（秒，已合并环境变量覆盖；0 = 关闭）。
    pub(crate) vector_reconcile_secs: u64,
    /// 实际生效的配置文件路径，日志里要打出来，现场排查第一步就是确认读的哪个文件。
    pub(crate) source: PathBuf,
}

/// 进程内配置快照：`load()` 成功后写入，供 `vector` 这类没有机会拿到 `Config` 的
/// 运行期组件读取（它们在测试里也会被调用，所以未初始化时各 getter 回落默认值）。
static CURRENT: std::sync::OnceLock<Config> = std::sync::OnceLock::new();

/// 已加载的配置；测试或纯库调用可能没有。
pub(crate) fn current() -> Option<&'static Config> {
    CURRENT.get()
}

/// 向量索引内存预算（字节）。未加载配置时用内置默认。
pub(crate) fn vector_index_budget_bytes() -> usize {
    let mb = current()
        .map(|c| c.vector_index_budget_mb)
        .filter(|mb| *mb > 0)
        .unwrap_or(DEFAULT_VECTOR_BUDGET_MB);
    (mb as usize).saturating_mul(1024 * 1024)
}

/// 后台对账周期（秒）；0 = 关闭。
pub(crate) fn vector_reconcile_secs() -> u64 {
    current()
        .map(|c| c.vector_reconcile_secs)
        .unwrap_or(DEFAULT_VECTOR_RECONCILE_SECS)
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    database: DatabaseSection,
    #[serde(default)]
    security: SecuritySection,
    #[serde(default)]
    vector: VectorSection,
}

#[derive(Debug, Default, Deserialize)]
struct DatabaseSection {
    url: Option<String>,
    auto_migrate: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct SecuritySection {
    master_key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct VectorSection {
    /// 内存预算（MB）。
    index_budget_mb: Option<u64>,
    /// 后台对账周期（秒）；0 = 关闭。
    reconcile_secs: Option<u64>,
}

/// 读取配置。**任何缺失都返回 `Err`**，由调用方打印并 `exit(非 0)`（D33）。
pub(crate) fn load() -> Result<Config, String> {
    let path = std::env::var(CONFIG_PATH_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));

    if !path.exists() {
        write_template(&path)?;
        return Err(format!(
            "配置文件不存在，已生成模板 {}。请填写 [database] url 后重新启动。",
            path.display()
        ));
    }

    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取配置文件 {} 失败：{e}", path.display()))?;
    let file: FileConfig =
        toml::from_str(&text).map_err(|e| format!("解析配置文件 {} 失败：{e}", path.display()))?;

    let from_env = std::env::var(DATABASE_URL_ENV).ok();
    let database_url = from_env
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| file.database.url.as_deref().map(str::trim))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "未配置数据库连接串：请设置环境变量 {DATABASE_URL_ENV}，或填写 {} 的 [database] url。",
                path.display()
            )
        })?
        .to_string();

    let config = Config {
        database_url,
        auto_migrate: file.database.auto_migrate.unwrap_or(true),
        master_key: std::env::var(MASTER_KEY_ENV)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                file.security
                    .master_key
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            }),
        // 环境变量 > 文件 > 默认（D34）。预算 0/非法值当"没填"，回落到默认 512；
        // 对账周期 0 是**有意义的**（关闭），不能当没填。
        vector_index_budget_mb: env_u64(VECTOR_BUDGET_ENV)
            .or(file.vector.index_budget_mb)
            .filter(|mb| *mb > 0)
            .unwrap_or(DEFAULT_VECTOR_BUDGET_MB),
        vector_reconcile_secs: env_u64(VECTOR_RECONCILE_ENV)
            .or(file.vector.reconcile_secs)
            .unwrap_or(DEFAULT_VECTOR_RECONCILE_SECS),
        source: path,
    };
    // 运行期组件（vector）通过 `current()` 读这份快照；重复 load 以第一次为准。
    let _ = CURRENT.set(config.clone());
    Ok(config)
}

/// 读一个 `u64` 环境变量；空/非法值当没填（不 fail-fast：这些是调优项，
/// 写错一个数字不该让服务起不来）。
fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
}

/// 首次运行生成模板。写盘走「临时文件 + rename」，避免半个文件被读进来。
fn write_template(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("创建配置目录 {} 失败：{e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, TEMPLATE).map_err(|e| format!("写入模板 {} 失败：{e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("重命名模板到 {} 失败：{e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_valid_toml_and_ships_empty_dsn() {
        let file: FileConfig = toml::from_str(TEMPLATE).expect("模板必须是合法 TOML");
        assert_eq!(file.database.url.as_deref(), Some(""));
        assert_eq!(file.database.auto_migrate, Some(true));
        // 主密钥留空 → security 自动生成 data/master.key。
        assert_eq!(file.security.master_key.as_deref(), Some(""));
    }

    /// B-27/B-28 的两个调优项要能从文件里配，并且模板里说明每个 KIKO_* 覆盖。
    #[test]
    fn template_documents_vector_knobs_and_all_env_overrides() {
        let file: FileConfig = toml::from_str(TEMPLATE).expect("模板必须是合法 TOML");
        assert_eq!(file.vector.index_budget_mb, Some(512));
        assert_eq!(file.vector.reconcile_secs, Some(60));
        for env in [
            CONFIG_PATH_ENV,
            DATABASE_URL_ENV,
            MASTER_KEY_ENV,
            VECTOR_BUDGET_ENV,
            VECTOR_RECONCILE_ENV,
        ] {
            assert!(TEMPLATE.contains(env), "模板必须写明环境变量 {env}");
        }
    }

    /// 没有配置文件（纯库/测试）时，运行期 getter 必须用内置默认，而不是 0。
    #[test]
    fn vector_getters_fall_back_to_defaults() {
        if current().is_none() {
            assert_eq!(vector_index_budget_bytes(), 512 * 1024 * 1024);
            assert_eq!(vector_reconcile_secs(), DEFAULT_VECTOR_RECONCILE_SECS);
        }
    }

    #[test]
    fn auto_migrate_defaults_to_true() {
        let file: FileConfig = toml::from_str("[database]\nurl = \"postgres://x\"").unwrap();
        assert_eq!(file.database.auto_migrate, None);
        // 缺省即 true（D45）——这里只验证解析层，真正的默认值在 load() 里。
        assert!(file.database.auto_migrate.unwrap_or(true));
    }
}
