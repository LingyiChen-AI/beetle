//! 所有落盘路径的唯一出口。设置环境变量 `BEETLE_HOME`（兼容旧名 `BUG_PATROL_HOME`）可把根目录整体改到别处（测试、开发模式用）。

use std::path::PathBuf;

const APP_DIR: &str = "Beetle";
/// 改名前（BUG纠察队）的目录名，首次启动迁移用（见 `rebrand.rs`）。
const LEGACY_APP_DIR: &str = "BugPatrol";

/// 根目录覆盖用的环境变量：新名优先，旧名兼容。
pub const HOME_ENV: &str = "BEETLE_HOME";
pub const LEGACY_HOME_ENV: &str = "BUG_PATROL_HOME";

fn override_root() -> Option<PathBuf> {
    [HOME_ENV, LEGACY_HOME_ENV]
        .iter()
        .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// 是否设置了根目录覆盖（开发 / 测试用的隔离目录）。
pub fn home_overridden() -> bool {
    override_root().is_some()
}

/// 配置目录：`%APPDATA%\Beetle`（放 config.json、runs.db）。macOS 为 `~/Library/Application Support/Beetle`。
pub fn config_dir() -> PathBuf {
    match override_root() {
        Some(root) => root.join("config"),
        None => dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR),
    }
}

/// 本地数据根目录：`%LOCALAPPDATA%\Beetle`。macOS 为 `~/Library/Caches/Beetle`。
pub fn data_dir() -> PathBuf {
    match override_root() {
        Some(root) => root.join("data"),
        None => data_base().join(APP_DIR),
    }
}

/// 改名前的配置目录 `%APPDATA%\BugPatrol`；设置了根目录覆盖时为 `<root>/legacy/config`（单测在这里构造旧目录）。
pub fn legacy_config_dir() -> PathBuf {
    match override_root() {
        Some(root) => root.join("legacy").join("config"),
        None => dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(LEGACY_APP_DIR),
    }
}

/// 改名前的本地数据目录 `%LOCALAPPDATA%\BugPatrol`（旧版各平台都用 `dirs::data_local_dir()`）；
/// 设置了根目录覆盖时为 `<root>/legacy/data`。
pub fn legacy_data_dir() -> PathBuf {
    match override_root() {
        Some(root) => root.join("legacy").join("data"),
        None => dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(LEGACY_APP_DIR),
    }
}

/// 本地数据目录的上一级：Windows `%LOCALAPPDATA%`，macOS `~/Library/Caches`，其他系统 `dirs::data_local_dir()`。
fn data_base() -> PathBuf {
    let base = if cfg!(target_os = "macos") { dirs::cache_dir() } else { dirs::data_local_dir() };
    base.unwrap_or_else(std::env::temp_dir)
}

/// Agent 工作目录根：`<data>/workspaces`，其下为 `<bot_id>/<group_slug>/`。
pub fn workspaces_dir() -> PathBuf {
    data_dir().join("workspaces")
}

/// 代码缓存目录：`<data>/code`。
pub fn code_cache_dir() -> PathBuf {
    data_dir().join("code")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_home;

    #[test]
    fn home_override_moves_all_dirs() {
        let home = test_home();
        let root = home.path().to_path_buf();
        assert!(config_dir().starts_with(&root));
        assert!(data_dir().starts_with(&root));
        assert_eq!(workspaces_dir(), data_dir().join("workspaces"));
        assert_eq!(code_cache_dir(), data_dir().join("code"));
    }
}
