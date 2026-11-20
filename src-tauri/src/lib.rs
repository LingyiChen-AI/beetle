pub mod agent;
pub mod catalog;
pub mod commands;
pub mod commands_catalog_view;
pub mod commands_ext;
pub mod commands_stats;
pub mod dbquery;
pub mod updater;
pub mod dispatcher;
pub mod envcheck;
pub mod feishu;
pub mod issue;
pub mod migrate;
pub mod onboard;
pub mod platform;
pub mod portable;
pub mod rebrand;
pub mod redact;
pub mod review_card;
pub mod runner;
pub mod skills;
pub mod store;
pub mod supervisor;
pub mod templates;
pub mod tray;
pub mod workspace;

#[cfg(test)]
pub(crate) mod test_support;

/// 开机自启时带的参数：此时只常驻托盘，不弹主窗口。
const AUTOSTART_ARG: &str = "--autostart";

/// 开机自启：首次运行默认开启，之后尊重用户的选择（关掉后不再自动打开）。
/// 设置了 `BEETLE_HOME` / 旧名 `BUG_PATROL_HOME`（开发 / 测试用的隔离目录）时不碰系统自启项，免得把调试版写进开机启动。
fn init_autostart(app: &tauri::AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    if platform::paths::home_overridden() {
        return;
    }
    let mut enable = false;
    if let Err(e) = store::config::update(|cfg| {
        enable = commands::first_run_autostart(cfg);
        Ok(())
    }) {
        eprintln!("beetle: 读取开机自启设置失败：{e}");
        return;
    }
    if enable && let Err(e) = app.autolaunch().enable() {
        eprintln!("beetle: 开启开机自启失败：{e}");
        // 没开成就撤回「已初始化」标记，下次启动再试；否则设置页一直显示「已开启」但系统里没有自启项
        if let Err(e) = store::config::update(|cfg| {
            commands::autostart_enable_failed(cfg);
            Ok(())
        }) {
            eprintln!("beetle: 撤回开机自启标记失败：{e}");
        }
    }
}

/// 已经在运行时又被启动（单实例插件转过来的参数）：开机自启那次（带 `--autostart`）不弹主窗口，用户手动打开才唤起。
pub fn should_show_on_second_launch(args: &[String]) -> bool {
    !args.iter().any(|a| a == AUTOSTART_ARG)
}

pub fn run() {
    tauri::Builder::default()
        // 单实例必须最先注册：第二次启动只唤起已有窗口。
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if should_show_on_second_launch(&args) {
                tray::show_main(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::UpdateState::default())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .on_window_event(|win, event| tray::handle_window_event(win, event))
        .invoke_handler(tauri::generate_handler![
            commands::list_bots,
            commands::save_bot,
            commands::delete_bot,
            commands::start_bot,
            commands::stop_bot,
            commands::set_start_with_client,
            commands::save_group,
            commands::remove_group,
            commands::list_projects,
            commands::save_project,
            commands::remove_project,
            commands::save_datasource,
            commands::delete_datasource,
            commands::test_datasource,
            commands::list_local_skills,
            commands::set_default_projects,
            commands::list_runs,
            commands::get_run,
            commands::cancel_run,
            commands::retry_run,
            commands::list_skills,
            commands::get_skill_settings,
            commands::save_skill_settings,
            commands::get_autostart,
            commands::set_autostart,
            commands_ext::env_check,
            commands_ext::env_requirements,
            commands_ext::get_runtime_settings,
            commands_ext::set_runtime_settings,
            commands_ext::env_fix,
            commands_ext::list_templates,
            commands_ext::get_template,
            commands_ext::save_template,
            commands_ext::reset_template,
            commands_ext::get_raw_log,
            commands_ext::get_appearance,
            commands_ext::set_appearance,
            commands_ext::bot_stats,
            commands_ext::export_config,
            commands_ext::preview_import,
            commands_ext::apply_import,
            commands_ext::detect_legacy,
            commands_ext::migrate_legacy,
            commands_ext::import_local_skill,
            catalog::catalog_list,
            catalog::catalog_install,
            catalog::catalog_uninstall,
            catalog::catalog_refresh_config,
            issue::check_issue_token,
            feishu::registration::feishu_create_app,
            feishu::registration::feishu_cancel_create_app,
            updater::update_status, updater::check_update, updater::install_update,
            commands_stats::stats_overview, commands_catalog_view::catalog_files, commands_catalog_view::catalog_read_file,
        ])
        .setup(|app| {
            use tauri::Manager;
            // 改名后首次启动：把 BUG纠察队 的配置、运行记录、凭据搬过来；再做配置升级（补 issue-record 技能）。必须在读配置之前。
            rebrand::run_startup_migrations();
            updater::spawn_periodic(app.handle().clone());
            if let Ok(dir) = app.path().resource_dir() {
                skills::init(&dir);
                catalog::init(&dir);
            }
            let handle = app.handle().clone();
            let store = std::sync::Arc::new(store::runs::RunStore::open(commands::runs_db_path())?);
            let interpreters = workspace::Interpreters {
                python: platform::locate::find("python").unwrap_or_else(|| "python".into()),
                node: platform::locate::find("node").unwrap_or_else(|| "node".into()),
            };
            let events = std::sync::Arc::new(commands::TauriSink { app: handle.clone() });
            let deps = supervisor::SupervisorDeps::production(store.clone(), interpreters, events);
            // Supervisor::new 要在 tokio 运行时里调用；它启动时已清理遗留运行和 30 天前的记录。
            let supervisor = tauri::async_runtime::block_on(async { supervisor::Supervisor::new(deps) });
            app.manage(commands::AppState { supervisor: supervisor.clone(), store });

            tray::create(&handle)?;
            init_autostart(&handle);
            let launched_by_autostart = std::env::args().any(|a| a == AUTOSTART_ARG);
            if !launched_by_autostart {
                tray::show_main(&handle);
            }
            // 启动勾了「随客户端启动」的机器人（读凭据库，放到线程里不卡界面）。
            // 与界面「启动」同一套环境检查：缺所选引擎 / 基础项的机器人不自动启动，状态显示缺哪项。
            std::thread::spawn(move || supervisor.start_autostart_bots(&envcheck::ensure_ready));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动 Beetle 失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_launch_by_autostart_does_not_show_window() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(should_show_on_second_launch(&args(&["beetle.exe"])));
        assert!(!should_show_on_second_launch(&args(&["beetle.exe", "--autostart"])));
    }
}
