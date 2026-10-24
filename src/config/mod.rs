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
/// 默认配置文件。仓库根的 `config.toml` 是 rustc 构建配置，两者不许重名（D34）。
pub(crate) const DEFAULT_CONFIG_PATH: &str = "data/kiko.toml";

const TEMPLATE: &str = r#"# Kiko 应用配置（D34）。
#
# 仓库根的 config.toml 是 rustc 构建配置，不是这个文件。
# 优先级：环境变量 > 本文件 > 内置默认；DSN 可被 KIKO_DATABASE_URL 覆盖。
#
# 首次运行缺少本文件时会生成这份模板并以非 0 退出（D33）：
# 填好 [database] url 后重启即可，没有 setup 向导。

[database]
# PostgreSQL 连接串，例如 postgres://kiko:password@127.0.0.1:5432/kiko_dev
# schema 固定为 kiko（D37），应用会自动创建，不需要写进连接串。
url = ""

# 启动时自动执行 migration；由 DBA 统一跑 DDL 的部署改成 false（D45）。
auto_migrate = true
"#;

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) database_url: String,
    pub(crate) auto_migrate: bool,
    /// 实际生效的配置文件路径，日志里要打出来，现场排查第一步就是确认读的哪个文件。
    pub(crate) source: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    database: DatabaseSection,
}

#[derive(Debug, Default, Deserialize)]
struct DatabaseSection {
    url: Option<String>,
    auto_migrate: Option<bool>,
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

    Ok(Config {
        database_url,
        auto_migrate: file.database.auto_migrate.unwrap_or(true),
        source: path,
    })
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
    }

    #[test]
    fn auto_migrate_defaults_to_true() {
        let file: FileConfig = toml::from_str("[database]\nurl = \"postgres://x\"").unwrap();
        assert_eq!(file.database.auto_migrate, None);
        // 缺省即 true（D45）——这里只验证解析层，真正的默认值在 load() 里。
        assert!(file.database.auto_migrate.unwrap_or(true));
    }
}
