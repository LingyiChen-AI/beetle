//! 新群自动接入：机器人主人在未登记的群里 @ 机器人时，把该群登记进配置。

use crate::feishu::lark_cli::LarkOps;
use crate::store::config::{self, AppConfig, Group, Trigger};

/// chat_id 的后 `n` 个字符（按字符取，chat_id 实际都是 ASCII）。
pub fn chat_tail(chat_id: &str, n: usize) -> String {
    let count = chat_id.chars().count();
    chat_id.chars().skip(count.saturating_sub(n)).collect()
}

/// 自动接入的群的工作目录名：`beetle-<chat_id 后 8 位>`。已登记的群沿用配置里的旧目录名。
pub fn group_slug(chat_id: &str) -> String {
    format!("beetle-{}", chat_tail(chat_id, 8))
}

/// 自动新建的记录文档标题。
pub fn doc_title(group_name: &str) -> String {
    format!("BUG自动检测机器人（{group_name}）")
}

/// 把群登记到机器人下并写回配置，返回登记的群和写入后的最新配置。
/// - 慢操作（lark-cli 取群名、建文档，每次最长 30 秒）在配置锁外做；写入走 `config::update`，
///   锁内重读配置、确认群仍未登记才追加，已登记则沿用已有记录（不覆盖，也不会冲掉别人同时写入的字段）。
/// - 调用前群已登记：直接返回，不建文档（同群连发两次 @ 时第二次走这里）。
/// - 群名取不到：报错，配置不动。
/// - 记录文档建不出来：照样登记（`doc_url` 为空，记录文档技能会提示未配置）。
/// - 谁能触发默认「仅机器人主人」，主人在界面里可改成所有人。
///
/// 同步函数，异步上下文里要放进 `spawn_blocking`。
pub fn register_group(bot_id: &str, chat_id: &str, lark: &dyn LarkOps) -> Result<(Group, AppConfig), String> {
    let find = |cfg: &AppConfig| -> Result<Option<Group>, String> {
        let bot = cfg.bots.iter().find(|b| b.id == bot_id).ok_or_else(|| format!("找不到机器人 {bot_id}"))?;
        Ok(bot.groups.iter().find(|g| g.chat_id == chat_id).cloned())
    };
    let current = config::load()?;
    if let Some(g) = find(&current)? {
        return Ok((g, current));
    }
    // 慢操作放在锁外
    let name = lark.chat_name(chat_id).map_err(|e| format!("取群名失败：{e}"))?;
    let doc_url = match lark.create_doc(&doc_title(&name)) {
        Ok(url) => Some(url),
        Err(e) => {
            eprintln!("beetle: 群 {chat_id} 新建记录文档失败，先不带文档接入：{e}");
            None
        }
    };
    let new_group = Group {
        chat_id: chat_id.to_string(),
        name,
        slug: group_slug(chat_id),
        projects: current.default_projects.clone(),
        doc_url,
        trigger: Trigger::Owner,
        all_projects: false,
    };
    let mut registered: Option<Group> = None;
    let cfg = config::update(|cfg| {
        let existing = find(cfg)?;
        let bot = cfg.bots.iter_mut().find(|b| b.id == bot_id).expect("find 已确认机器人存在");
        match existing {
            Some(g) => {
                eprintln!("beetle: 群 {chat_id} 在接入期间已被登记，沿用已有记录");
                registered = Some(g);
            }
            None => {
                bot.groups.push(new_group.clone());
                registered = Some(new_group);
            }
        }
        Ok(())
    })?;
    Ok((registered.expect("update 成功时一定已赋值"), cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::config::{self, Bot};
    use crate::test_support::{FakeLark, test_home};

    fn save_cfg_with_bot() {
        let mut cfg = AppConfig::default();
        cfg.bots.push(Bot::new_default("b1", "机器人", "cli_x"));
        cfg.default_projects = vec!["demo-service".into()];
        config::save(&cfg).unwrap();
    }

    #[test]
    fn onboard_slug_and_title_format() {
        assert_eq!(group_slug("oc_a1b2c3d4e5f6a7b8"), "beetle-e5f6a7b8");
        assert_eq!(group_slug("oc_short"), "beetle-oc_short");
        assert_eq!(doc_title("示例项目群"), "BUG自动检测机器人（示例项目群）");
        assert_eq!(chat_tail("oc_1234567890", 8), "34567890");
    }

    #[test]
    fn register_group_uses_chat_name_default_projects_and_new_doc() {
        let _home = test_home();
        save_cfg_with_bot();
        let lark = FakeLark::default();
        let (g, cfg) = register_group("b1", "oc_a1b2c3d4e5f6a7b8", &lark).unwrap();
        assert_eq!(g.name, "新接入群");
        assert_eq!(g.slug, "beetle-e5f6a7b8");
        assert_eq!(g.projects, vec!["demo-service"]);
        assert_eq!(g.doc_url.as_deref(), Some("https://x.feishu.cn/docx/new_doc"));
        assert_eq!(*lark.docs.lock().unwrap(), vec!["BUG自动检测机器人（新接入群）"]);
        assert_eq!(cfg.bots[0].groups, vec![g.clone()]);
        assert_eq!(config::load().unwrap().bots[0].groups, vec![g]);
    }

    #[test]
    fn onboarded_group_defaults_to_owner_only() {
        let _home = test_home();
        save_cfg_with_bot();
        let (g, _) = register_group("b1", "oc_x12345678", &FakeLark::default()).unwrap();
        assert_eq!(g.trigger, Trigger::Owner);
    }

    #[test]
    fn register_existing_group_returns_it_without_new_doc() {
        let _home = test_home();
        save_cfg_with_bot();
        let lark = FakeLark::default();
        let (first, _) = register_group("b1", "oc_x12345678", &lark).unwrap();
        let (again, cfg) = register_group("b1", "oc_x12345678", &lark).unwrap();
        assert_eq!(first, again);
        assert_eq!(lark.docs.lock().unwrap().len(), 1);
        assert_eq!(cfg.bots[0].groups.len(), 1);
    }

    /// 取群名期间（锁外）别处写了配置：既不能被接入冲掉，群登记过了也不能被覆盖。
    struct WriterLark {
        inner: FakeLark,
        during: Box<dyn Fn() + Send + Sync>,
    }

    impl LarkOps for WriterLark {
        fn current_user(&self) -> Result<crate::feishu::lark_cli::UserIds, String> {
            self.inner.current_user()
        }
        fn chat_name(&self, chat_id: &str) -> Result<String, String> {
            (self.during)();
            self.inner.chat_name(chat_id)
        }
        fn user_name(&self, union_id: &str) -> Result<String, String> {
            self.inner.user_name(union_id)
        }
        fn message_text(&self, message_id: &str) -> Result<String, String> {
            self.inner.message_text(message_id)
        }
        fn create_doc(&self, title: &str) -> Result<String, String> {
            self.inner.create_doc(title)
        }
    }

    #[test]
    fn concurrent_update_during_onboard_is_kept() {
        let _home = test_home();
        save_cfg_with_bot();
        let lark = WriterLark {
            inner: FakeLark::default(),
            during: Box::new(|| {
                config::update(|c| {
                    c.owner_open_id = Some("ou_owner".into());
                    c.bots[0].timeout_minutes = 45;
                    Ok(())
                })
                .unwrap();
            }),
        };
        let (g, cfg) = register_group("b1", "oc_x12345678", &lark).unwrap();
        let saved = config::load().unwrap();
        assert_eq!(saved, cfg);
        assert_eq!(saved.owner_open_id.as_deref(), Some("ou_owner"));
        assert_eq!(saved.bots[0].timeout_minutes, 45);
        assert_eq!(saved.bots[0].groups, vec![g]);
    }

    #[test]
    fn group_registered_meanwhile_is_not_overwritten() {
        let _home = test_home();
        save_cfg_with_bot();
        let lark = WriterLark {
            inner: FakeLark::default(),
            during: Box::new(|| {
                config::update(|c| {
                    c.bots[0].groups.push(Group {
                        chat_id: "oc_x12345678".into(),
                        name: "主人手动登记".into(),
                        slug: "manual".into(),
                        projects: vec![],
                        doc_url: None,
                        trigger: Trigger::All,
                        all_projects: false,
                    });
                    Ok(())
                })
                .unwrap();
            }),
        };
        let (g, _) = register_group("b1", "oc_x12345678", &lark).unwrap();
        assert_eq!(g.name, "主人手动登记");
        assert_eq!(config::load().unwrap().bots[0].groups, vec![g]);
    }

    #[test]
    fn chat_name_failure_is_an_error_and_config_untouched() {
        let _home = test_home();
        save_cfg_with_bot();
        let lark = FakeLark { chat_name: Err("lark-cli 调用失败：未登录".into()), ..FakeLark::default() };
        let err = register_group("b1", "oc_x12345678", &lark).unwrap_err();
        assert!(err.contains("群名"), "{err}");
        assert!(config::load().unwrap().bots[0].groups.is_empty());
        assert!(lark.docs.lock().unwrap().is_empty());
    }

    #[test]
    fn doc_failure_still_registers_group_without_doc() {
        let _home = test_home();
        save_cfg_with_bot();
        let lark = FakeLark { doc_url: Err("没有权限".into()), ..FakeLark::default() };
        let (g, cfg) = register_group("b1", "oc_x12345678", &lark).unwrap();
        assert_eq!(g.doc_url, None);
        assert_eq!(cfg.bots[0].groups.len(), 1);
    }

    #[test]
    fn unknown_bot_is_an_error() {
        let _home = test_home();
        save_cfg_with_bot();
        assert!(register_group("nope", "oc_x", &FakeLark::default()).is_err());
    }
}
