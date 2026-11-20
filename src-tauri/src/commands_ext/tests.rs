use super::*;
use crate::platform::paths;
use crate::store::config::Engine;
use crate::test_support::test_home;

fn rec(id: &str, status: RunStatus, started_at: i64, finished_at: Option<i64>) -> RunRecord {
    RunRecord {
        id: id.into(),
        bot_id: "b1".into(),
        chat_id: "oc_1".into(),
        group_name: "群".into(),
        trigger_message_id: "om_1".into(),
        sender_name: "张三".into(),
        text: "查一下".into(),
        engine: Engine::Claude,
        engine_version: None,
        session_id: None,
        status,
        started_at,
        finished_at,
        final_reply: None,
        error: None,
        sql_count: 0,
        reply_message_id: None,
        sender_union_id: None,
    }
}

#[test]
fn list_templates_shows_builtins_then_custom_and_modified_flag() {
    let _home = test_home();
    let list = list_templates_impl().unwrap();
    assert_eq!(list.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["bug-hunt", "qa", "code-review"]);
    assert!(list.iter().all(|t| t.builtin && !t.modified));

    save_template_impl("bug-hunt", "改过的 {{GROUP_NAME}}").unwrap();
    save_template_impl("my-tpl", "自定义 {{CHAT_ID}}").unwrap();
    let list = list_templates_impl().unwrap();
    let find = |n: &str| list.iter().find(|t| t.name == n).unwrap().clone();
    assert_eq!(find("bug-hunt"), TemplateInfo { name: "bug-hunt".into(), builtin: true, modified: true });
    assert_eq!(find("qa").modified, false);
    assert_eq!(find("my-tpl"), TemplateInfo { name: "my-tpl".into(), builtin: false, modified: true });
}

#[test]
fn get_save_reset_template_roundtrip() {
    let _home = test_home();
    let builtin = templates::load("qa").unwrap();
    save_template_impl("qa", "只回答 {{GROUP_NAME}}").unwrap();
    assert_eq!(templates::load("qa").unwrap(), "只回答 {{GROUP_NAME}}");
    reset_template_impl("qa").unwrap();
    assert_eq!(templates::load("qa").unwrap(), builtin);
    // 重复恢复不报错。
    reset_template_impl("qa").unwrap();
    // 不留临时文件。
    let tmp = std::fs::read_dir(templates::user_dir())
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count())
        .unwrap_or(0);
    assert_eq!(tmp, 0);
}

#[test]
fn save_template_rejects_unknown_variable_and_bad_input() {
    let _home = test_home();
    let err = save_template_impl("qa", "你好 {{NOPE}}").unwrap_err();
    assert!(err.contains("未替换的变量") && err.contains("NOPE"), "{err}");
    assert!(save_template_impl("qa", "   ").unwrap_err().contains("不能为空"));
    assert!(save_template_impl("../evil", "x").unwrap_err().contains("不合法"));
    assert!(reset_template_impl("a/b").is_err());
    // 校验失败不落盘。
    assert!(!templates::user_dir().join("qa.md").exists());
    // 全部已知变量都能通过校验。
    let all = templates::VAR_KEYS.iter().map(|k| format!("{{{{{k}}}}}")).collect::<Vec<_>>().join(" ");
    save_template_impl("all-vars", &all).unwrap();
}

#[test]
fn raw_log_returns_both_files_and_tolerates_missing() {
    let _home = test_home();
    assert_eq!(get_raw_log_impl("run-1").unwrap(), RawLog { stdout: String::new(), stderr: String::new() });
    std::fs::create_dir_all(paths::data_dir().join("runs")).unwrap();
    std::fs::write(runner::raw_log_path("run-1"), "{\"a\":1}\n{\"b\":2}\n").unwrap();
    std::fs::write(runner::stderr_log_path("run-1"), "出错了\n").unwrap();
    let log = get_raw_log_impl("run-1").unwrap();
    assert_eq!(log.stdout, "{\"a\":1}\n{\"b\":2}\n");
    assert_eq!(log.stderr, "出错了\n");
}

#[test]
fn raw_log_keeps_only_the_tail_and_drops_the_partial_first_line() {
    let _home = test_home();
    std::fs::create_dir_all(paths::data_dir().join("runs")).unwrap();
    let line = format!("{}\n", "x".repeat(99)); // 100 字节
    let mut text = String::new();
    for i in 0..30 {
        text.push_str(&format!("{i:02}{}\n", "y".repeat(97)));
    }
    let p = paths::data_dir().join("runs").join("big.jsonl");
    std::fs::write(&p, &text).unwrap();
    let tail = read_tail(&p, 1050).unwrap();
    assert!(tail.len() <= 1050 && tail.len() >= 900, "{}", tail.len());
    assert!(tail.starts_with(|c: char| c.is_ascii_digit()), "第一行必须是完整的一行");
    assert!(tail.ends_with(&format!("29{}\n", "y".repeat(97))));
    assert_eq!(line.len(), 100);
}

#[test]
fn raw_log_rejects_path_traversal() {
    let _home = test_home();
    assert!(get_raw_log_impl("../config").is_err());
    assert!(get_raw_log_impl("").is_err());
}

#[test]
fn appearance_defaults_then_saves() {
    let _home = test_home();
    let a = config::load().unwrap().appearance;
    assert_eq!((a.mode, a.accent), (AppearanceMode::Dark, Accent::Vermilion));
    set_appearance_impl(Appearance { mode: AppearanceMode::System, accent: Accent::Purple }).unwrap();
    let a = config::load().unwrap().appearance;
    assert_eq!((a.mode, a.accent), (AppearanceMode::System, Accent::Purple));
    // 只改外观，不动别的配置。
    config::update(|c| {
        c.proxy = Some("http://p".into());
        Ok(())
    })
    .unwrap();
    set_appearance_impl(Appearance { mode: AppearanceMode::Light, accent: Accent::Teal }).unwrap();
    assert_eq!(config::load().unwrap().proxy.as_deref(), Some("http://p"));
}

#[test]
fn appearance_json_shape_matches_contract() {
    let v = serde_json::to_value(Appearance { mode: AppearanceMode::System, accent: Accent::Indigo }).unwrap();
    assert_eq!(v, serde_json::json!({"mode": "system", "accent": "indigo"}));
    assert!(serde_json::from_value::<Appearance>(serde_json::json!({"mode": "neon"})).is_err());
}

#[test]
fn bot_stats_counts_only_today_and_ignores_cancelled() {
    let records = vec![
        rec("a", RunStatus::Replied, 10_000, Some(14_000)),
        rec("b", RunStatus::Replied, 11_000, Some(13_000)),
        rec("c", RunStatus::Failed, 12_000, Some(12_500)),
        rec("d", RunStatus::Cancelled, 12_000, Some(12_100)),
        rec("e", RunStatus::Running, 12_000, None),
        rec("old", RunStatus::Failed, 500, Some(600)),
    ];
    let s = bot_stats_from(&records, 1_000);
    assert_eq!(s.today_runs, 5);
    assert_eq!(s.today_replied, 2);
    assert!((s.success_rate - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(s.avg_duration_ms, 3_000);
    let empty = bot_stats_from(&[], 0);
    assert_eq!((empty.today_runs, empty.success_rate, empty.avg_duration_ms), (0, 0.0, 0));
}

#[test]
fn bot_stats_reads_from_the_store() {
    let home = test_home();
    let store = RunStore::open(home.path().join("runs.db")).unwrap();
    store.insert_run(&rec("r1", RunStatus::Replied, 5_000, Some(6_000))).unwrap();
    let mut other = rec("r2", RunStatus::Replied, 5_000, Some(6_000));
    other.bot_id = "b2".into();
    store.insert_run(&other).unwrap();
    let s = bot_stats_impl(&store, "b1", 0).unwrap();
    assert_eq!((s.today_runs, s.avg_duration_ms), (1, 1_000));
}

// ---------- phase23-issues 第 15 条：设置页的代理 / 代码缓存目录 / 日志保留天数 ----------

fn rs(proxy: Option<&str>, dir: Option<&str>, days: u32) -> RuntimeSettings {
    RuntimeSettings { proxy: proxy.map(String::from), code_cache_dir: dir.map(String::from), log_retention_days: days }
}

#[test]
fn runtime_settings_default_view() {
    let _home = test_home();
    let v = runtime_settings_view(&config::load().unwrap());
    assert_eq!((v.proxy, v.code_cache_dir, v.log_retention_days), (None, None, 30));
    assert_eq!(v.default_code_cache_dir, paths::code_cache_dir().to_string_lossy().replace('\\', "/"));
    assert_eq!((v.min_retention_days, v.max_retention_days), (1, 3650));
}

#[test]
fn runtime_settings_save_and_read_back() {
    let home = test_home();
    let dir = home.path().join("cache").join("code");
    let dir_s = dir.to_string_lossy().into_owned();
    set_runtime_settings_impl(rs(Some(" http://127.0.0.1:7890 "), Some(&dir_s), 7)).unwrap();
    assert!(dir.is_dir(), "保存时建好目录");
    let cfg = config::load().unwrap();
    assert_eq!(cfg.proxy.as_deref(), Some("http://127.0.0.1:7890"));
    assert_eq!(cfg.code_cache_dir(), std::path::PathBuf::from(dir_s.replace('\\', "/")));
    assert_eq!(cfg.log_retention_days(), 7);
    let v = runtime_settings_view(&cfg);
    assert_eq!(v.log_retention_days, 7);
    // 清空 = 回到默认
    set_runtime_settings_impl(rs(Some(""), Some("  "), 30)).unwrap();
    let cfg = config::load().unwrap();
    assert_eq!((cfg.proxy.clone(), cfg.code_cache_dir.clone()), (None, None));
    assert_eq!(cfg.code_cache_dir(), paths::code_cache_dir());
}

#[test]
fn runtime_settings_reject_bad_values_without_saving() {
    let _home = test_home();
    for bad in ["127.0.0.1:7890", "ftp://x:1", "http://"] {
        let err = set_runtime_settings_impl(rs(Some(bad), None, 30)).unwrap_err();
        assert!(err.contains("代理地址不合法"), "{bad}: {err}");
    }
    assert!(set_runtime_settings_impl(rs(Some("socks5://127.0.0.1:1080"), None, 30)).is_ok());
    assert!(set_runtime_settings_impl(rs(None, Some("relative/dir"), 30)).unwrap_err().contains("完整路径"));
    assert!(set_runtime_settings_impl(rs(None, None, 0)).unwrap_err().contains("保留天数"));
    assert!(set_runtime_settings_impl(rs(None, None, 4000)).unwrap_err().contains("保留天数"));
    assert_eq!(config::load().unwrap().proxy.as_deref(), Some("socks5://127.0.0.1:1080"), "失败的保存不改配置");
}

/// 日志保留天数作用于客户端启动时的清理（Supervisor::new 按配置天数清理运行记录）。
#[test]
fn startup_purge_uses_configured_retention_days() {
    use crate::supervisor::{EventSink, Supervisor, SupervisorDeps};
    struct Nop;
    impl EventSink for Nop {
        fn bot_state(&self, _: crate::supervisor::BotStateEvent) {}
        fn run_updated(&self, _: crate::supervisor::RunUpdatedEvent) {}
        fn run_step(&self, _: crate::supervisor::RunStepEvent) {}
    }
    let home = test_home();
    set_runtime_settings_impl(rs(None, None, 3)).unwrap();
    let store = Arc::new(RunStore::open(home.path().join("runs.db")).unwrap());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let day = 86_400_000;
    store.insert_run(&rec("five-days", RunStatus::Replied, now - 5 * day, Some(now - 5 * day))).unwrap();
    store.insert_run(&rec("one-day", RunStatus::Replied, now - day, Some(now - day))).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let runner = Arc::new(crate::runner::RunnerDeps {
        store: store.clone(),
        engines: std::collections::HashMap::new(),
        locate: Arc::new(|_| None),
        interpreters: crate::workspace::Interpreters { python: "python".into(), node: "node".into() },
    });
    let _sup = rt.block_on(async {
        Supervisor::new(SupervisorDeps {
            runner,
            transport: Arc::new(|_b: &config::Bot| Err("x".to_string())),
            lark: Arc::new(crate::test_support::FakeLark::default()),
            events: Arc::new(Nop),
        })
    });
    assert!(store.get_run("five-days").unwrap().is_none(), "超过 3 天的被清理");
    assert!(store.get_run("one-day").unwrap().is_some());
}
