//! 自动更新：启动后检查一次，之后每 30 分钟检查一次 GitHub Release 上的 `latest.json`（tauri-plugin-updater，更新包有签名校验）。
//! 发现新版本只通知界面（事件 `update-available`），由用户点「立即更新」才下载安装并重启，不会在机器人运行中途自动重启。

use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// 自动检查间隔。
pub const CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// 启动后等这么久再做第一次检查（避开启动时的网络和磁盘高峰）。
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
pub const EVENT: &str = "update-available";
/// 下载进度：`{downloaded, total}`（字节；total 可能为 null）。
pub const PROGRESS_EVENT: &str = "update-progress";

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct UpdateStatus {
    pub current: String,
    /// 有新版本时为新版本号。
    pub available: Option<String>,
    pub notes: Option<String>,
    /// 上次检查时间（毫秒时间戳）；还没检查过为 None。
    pub checked_at: Option<i64>,
    /// 上次检查失败的原因（网络不通等），成功时为 None。
    pub error: Option<String>,
}

#[derive(Default)]
pub struct UpdateState(pub Mutex<UpdateStatus>);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 检查一次并更新状态；有新版本时发事件。返回最新状态。
pub async fn check_once<R: Runtime>(app: &AppHandle<R>) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    let result = match app.updater() {
        Ok(u) => u.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let status = match result {
        Ok(Some(u)) => UpdateStatus { current, available: Some(u.version.clone()), notes: u.body.clone(), checked_at: Some(now_ms()), error: None },
        Ok(None) => UpdateStatus { current, available: None, notes: None, checked_at: Some(now_ms()), error: None },
        Err(e) => {
            // 检查失败不清掉已知的新版本，只记原因
            let prev = app.state::<UpdateState>().0.lock().map(|s| s.clone()).unwrap_or_default();
            UpdateStatus { current, checked_at: Some(now_ms()), error: Some(format!("检查更新失败：{e}")), ..prev }
        }
    };
    if let Ok(mut s) = app.state::<UpdateState>().0.lock() {
        *s = status.clone();
    }
    // 先释放锁再发事件
    if status.available.is_some() {
        let _ = app.emit(EVENT, &status);
    }
    status
}

/// 启动后台定时检查（开发版不检查，避免拿 0.x 的开发构建去比线上版本）。
pub fn spawn_periodic<R: Runtime>(app: AppHandle<R>) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            check_once(&app).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

#[tauri::command]
pub async fn update_status(app: AppHandle) -> Result<UpdateStatus, String> {
    let mut s = app.state::<UpdateState>().0.lock().map_err(|e| e.to_string())?.clone();
    s.current = app.package_info().version.to_string();
    Ok(s)
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<UpdateStatus, String> {
    Ok(check_once(&app).await)
}

/// 下载并安装新版本，完成后重启客户端。Windows 上安装程序会先结束本进程。
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| format!("检查更新失败：{e}"))?
        .ok_or("已经是最新版本")?;
    // 下载进度推给界面（每多下 1% 左右推一次，避免事件太密）
    let mut downloaded: u64 = 0;
    let mut last_pct: i64 = -1;
    let progress_app = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let pct = total.filter(|t| *t > 0).map(|t| (downloaded * 100 / t) as i64).unwrap_or(-1);
                if pct != last_pct {
                    last_pct = pct;
                    let _ = progress_app.emit(PROGRESS_EVENT, serde_json::json!({ "downloaded": downloaded, "total": total }));
                }
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载或安装更新失败：{e}"))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_30_minutes() {
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(1800));
    }
}
