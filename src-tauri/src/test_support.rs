//! 测试辅助：把 `BEETLE_HOME` 指到临时目录（同时清掉旧名 `BUG_PATROL_HOME`）。环境变量是进程级的，
//! 所以用全局锁把所有用到它的测试串行化。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static HOME_LOCK: Mutex<()> = Mutex::new(());

pub struct TestHome {
    dir: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl TestHome {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        // SAFETY: 持有 HOME_LOCK，期间没有其他测试读写该变量。
        unsafe { std::env::remove_var(crate::platform::paths::HOME_ENV) };
    }
}

/// 假 lark-cli：返回固定值，并记录新建过的文档标题。
pub struct FakeLark {
    pub owner: Result<crate::feishu::lark_cli::UserIds, String>,
    pub chat_name: Result<String, String>,
    pub doc_url: Result<String, String>,
    pub docs: Mutex<Vec<String>>,
    /// 查过姓名的用户 id（按 union_id 查）。
    pub user_lookups: Mutex<Vec<String>>,
}

impl Default for FakeLark {
    fn default() -> Self {
        FakeLark {
            owner: Ok(crate::feishu::lark_cli::UserIds { open_id: "ou_owner".into(), union_id: "on_owner".into() }),
            chat_name: Ok("新接入群".into()),
            doc_url: Ok("https://x.feishu.cn/docx/new_doc".into()),
            docs: Mutex::new(Vec::new()),
            user_lookups: Mutex::new(Vec::new()),
        }
    }
}

impl crate::feishu::lark_cli::LarkOps for FakeLark {
    fn current_user(&self) -> Result<crate::feishu::lark_cli::UserIds, String> {
        self.owner.clone()
    }
    fn chat_name(&self, _chat_id: &str) -> Result<String, String> {
        self.chat_name.clone()
    }
    fn user_name(&self, union_id: &str) -> Result<String, String> {
        self.user_lookups.lock().unwrap().push(union_id.to_string());
        Ok(format!("姓名-{union_id}"))
    }
    fn message_text(&self, message_id: &str) -> Result<String, String> {
        Ok(format!("内容-{message_id}"))
    }
    fn create_doc(&self, title: &str) -> Result<String, String> {
        self.docs.lock().unwrap().push(title.to_string());
        self.doc_url.clone()
    }
}

pub fn test_home() -> TestHome {
    let guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: 同上，由 HOME_LOCK 串行化。
    unsafe {
        std::env::remove_var(crate::platform::paths::LEGACY_HOME_ENV);
        std::env::set_var(crate::platform::paths::HOME_ENV, dir.path());
    }
    TestHome { dir, _guard: guard }
}
