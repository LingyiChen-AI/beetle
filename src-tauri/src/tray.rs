//! 托盘常驻：关窗口只是隐藏；托盘菜单可打开主窗口、逐个机器人启停、退出。

use crate::commands::AppState;
use crate::store::config;
use crate::supervisor::BotState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, Window, WindowEvent};

const TRAY_ID: &str = "main";
const SHOW_ID: &str = "show";
const QUIT_ID: &str = "quit";
const BOT_PREFIX: &str = "bot:";
/// 退出前给「取消中的运行」留一点时间结束进程树、回复取消文案。
const QUIT_GRACE: Duration = Duration::from_millis(800);

/// 托盘图标：Windows 用彩色 beetle-8（米色圆角底 + 朱红甲虫，深浅色任务栏和浅色溢出面板上都看得见）；
/// 有机器人异常时换成右下角叠白边朱红点的版本。macOS 菜单栏仍用黑色模板图标（系统按明暗自动着色），不区分异常。
/// 图标由 docs/logo/v4/beetle-8.svg 经 Edge 无头截图（透明底 512px）再缩到 64px 生成。
#[cfg(target_os = "macos")]
const TRAY_NORMAL: &[u8] = include_bytes!("../icons/tray/tray-template.png");
#[cfg(not(target_os = "macos"))]
const TRAY_NORMAL: &[u8] = include_bytes!("../icons/tray/tray-color.png");
#[cfg(target_os = "macos")]
const TRAY_ALERT: &[u8] = TRAY_NORMAL;
#[cfg(not(target_os = "macos"))]
const TRAY_ALERT: &[u8] = include_bytes!("../icons/tray/tray-color-alert.png");

static QUITTING: AtomicBool = AtomicBool::new(false);

/// 显示、取消最小化并聚焦主窗口。
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn state_text(state: &BotState) -> String {
    match state {
        BotState::Stopped => "已停止".into(),
        BotState::Connecting => "连接中".into(),
        BotState::Running => "运行中".into(),
        BotState::Error(_) => "异常".into(),
    }
}

/// 托盘里每个机器人一行：`● 名称 · 状态`。
pub fn bot_label(name: &str, state: &BotState) -> String {
    format!("● {name} · {}", state_text(state))
}

/// 点击机器人那一行：连接任务不在（已停止、启动失败、应用被删）→ 启动；
/// 任务还在（连接中、运行中、「异常：连接失败，正在重试」）→ 停止。
pub fn click_should_start(state: &BotState, alive: bool) -> bool {
    !alive || matches!(state, BotState::Stopped)
}

/// 是否有机器人处于异常状态（决定托盘用哪张图标）。
pub fn any_error<'a>(states: impl IntoIterator<Item = &'a BotState>) -> bool {
    states.into_iter().any(|s| matches!(s, BotState::Error(_)))
}

fn tray_image(alert: bool) -> Option<Image<'static>> {
    Image::from_bytes(if alert { TRAY_ALERT } else { TRAY_NORMAL }).ok()
}

fn current_alert<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(state) = app.try_state::<AppState>() else { return false };
    let bots = config::load().map(|c| c.bots).unwrap_or_default();
    let states: Vec<BotState> = bots.iter().map(|b| state.supervisor.state(&b.id)).collect();
    any_error(states.iter())
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, SHOW_ID, "打开主窗口", true, None::<&str>)?)?;
    let bots = config::load().map(|c| c.bots).unwrap_or_default();
    if !bots.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        for bot in &bots {
            let state = app.try_state::<AppState>().map(|s| s.supervisor.state(&bot.id)).unwrap_or(BotState::Stopped);
            let id = format!("{BOT_PREFIX}{}", bot.id);
            menu.append(&MenuItem::with_id(app, id, bot_label(&bot.name, &state), true, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, QUIT_ID, "退出", true, None::<&str>)?)?;
    Ok(menu)
}

/// 重建托盘菜单（机器人增删改、状态变化时调用）。托盘还没建好时什么都不做。
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
            if let Some(icon) = tray_image(current_alert(app)) {
                let _ = tray.set_icon(Some(icon));
                #[cfg(target_os = "macos")]
                let _ = tray.set_icon_as_template(true);
            }
        }
        Err(e) => eprintln!("beetle: 刷新托盘菜单失败：{e}"),
    }
}

fn toggle_bot<R: Runtime>(app: &AppHandle<R>, bot_id: String) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let supervisor = state.supervisor.clone();
    // start 要读凭据库、读配置，放到线程里，别卡托盘事件循环。
    std::thread::spawn(move || {
        if click_should_start(&supervisor.state(&bot_id), supervisor.is_alive(&bot_id)) {
            // 与界面「启动」同一套环境检查；不通过时状态显示缺哪项（托盘变成带红点的异常图标）。
            if let Err(e) = supervisor.start_checked(&bot_id, &crate::envcheck::ensure_ready) {
                eprintln!("beetle: 托盘启动机器人失败：{e}");
            }
        } else {
            supervisor.stop(&bot_id);
        }
    });
}

/// 退出：先停掉所有机器人（取消排队 / 运行中的任务、断开飞书连接），稍等一下再结束进程。
pub fn quit_gracefully<R: Runtime>(app: &AppHandle<R>) {
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(state) = app.try_state::<AppState>() {
            for bot in config::load().map(|c| c.bots).unwrap_or_default() {
                state.supervisor.stop(&bot.id);
            }
            std::thread::sleep(QUIT_GRACE);
        }
        app.exit(0);
    });
}

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Beetle")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                SHOW_ID => show_main(app),
                QUIT_ID => quit_gracefully(app),
                other => {
                    if let Some(bot_id) = other.strip_prefix(BOT_PREFIX) {
                        toggle_bot(app, bot_id.to_string());
                    }
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = tray_image(current_alert(app)) {
        builder = builder.icon(icon);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }
    builder.build(app)?;
    Ok(())
}

/// 关闭按钮只是隐藏到托盘；退出走托盘菜单。
pub fn handle_window_event<R: Runtime>(win: &Window<R>, event: &WindowEvent) {
    if win.label() != "main" {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = win.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_shows_name_and_state_text() {
        assert_eq!(bot_label("BUG纠察队", &BotState::Running), "● BUG纠察队 · 运行中");
        assert_eq!(bot_label("甲", &BotState::Stopped), "● 甲 · 已停止");
        assert_eq!(bot_label("乙", &BotState::Connecting), "● 乙 · 连接中");
        assert_eq!(bot_label("丙", &BotState::Error("连不上".into())), "● 丙 · 异常");
    }

    #[test]
    fn alert_only_when_some_bot_is_in_error() {
        assert!(!any_error([&BotState::Stopped, &BotState::Running, &BotState::Connecting]));
        assert!(any_error([&BotState::Running, &BotState::Error("x".into())]));
        assert!(!any_error(std::iter::empty::<&BotState>()));
    }

    #[test]
    fn tray_icons_decode() {
        assert!(tray_image(false).is_some());
        assert!(tray_image(true).is_some());
    }

    /// phase23-issues 第 16 条：Windows 托盘是彩色图标（不是纯白单色，浅色溢出面板上也看得见），异常版本不同。
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn windows_tray_icons_are_colored_and_alert_differs() {
        let img = tray_image(false).unwrap();
        let px = img.rgba();
        let colored = px.chunks(4).any(|p| p[3] > 200 && p[0] > 200 && p[1] < 120 && p[2] < 80);
        assert!(colored, "应含朱红色像素");
        let white_only = px.chunks(4).filter(|p| p[3] > 0).all(|p| p[0] > 240 && p[1] > 240 && p[2] > 240);
        assert!(!white_only, "不能是纯白单色图标");
        assert_ne!(TRAY_NORMAL, TRAY_ALERT);
        assert_eq!((img.width(), img.height()), (64, 64));
    }

    #[test]
    fn click_starts_stopped_or_failed_bots_and_stops_active_ones() {
        assert!(click_should_start(&BotState::Stopped, false));
        // 启动失败 / 应用被删：连接任务已结束 → 启动
        assert!(click_should_start(&BotState::Error("x".into()), false));
        // 审查 B2：异常但还在重试 → 停止（之前是空操作的「启动」）
        assert!(!click_should_start(&BotState::Error("连接失败，正在重试：x".into()), true));
        assert!(!click_should_start(&BotState::Connecting, true));
        assert!(!click_should_start(&BotState::Running, true));
    }
}
