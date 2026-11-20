//! App Secret 只存系统凭据库，config.json 里不出现密钥。

const SERVICE: &str = "dev.beetle.app";

fn entry_in(service: &str, app_id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(service, &format!("feishu:{app_id}")).map_err(|e| format!("打开凭据库失败：{e}"))
}

fn entry(app_id: &str) -> Result<keyring::Entry, String> {
    entry_in(SERVICE, app_id)
}

/// 读指定 service 下的 App Secret：Ok(None) = 没有这个条目。
pub fn secret_in(service: &str, app_id: &str) -> Result<Option<String>, String> {
    match entry_in(service, app_id)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("读取 App Secret 失败：{e}")),
    }
}

/// 把旧 service 下的 App Secret 复制到当前 service。新 service 已有就不覆盖。
/// 返回 Ok(true) = 复制了；Ok(false) = 旧的没有或新的已有，什么都没做。
pub fn copy_from_service(old_service: &str, app_id: &str) -> Result<bool, String> {
    if secret_in(SERVICE, app_id)?.is_some() {
        return Ok(false);
    }
    match secret_in(old_service, app_id)? {
        Some(secret) => set_app_secret(app_id, &secret).map(|()| true),
        None => Ok(false),
    }
}

pub fn set_app_secret(app_id: &str, secret: &str) -> Result<(), String> {
    entry(app_id)?
        .set_password(secret)
        .map_err(|e| format!("保存 App Secret 失败：{e}"))
}

pub fn app_secret(app_id: &str) -> Result<String, String> {
    entry(app_id)?.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => format!("未找到 {app_id} 的 App Secret，请在客户端里重新填写"),
        e => format!("读取 App Secret 失败：{e}"),
    })
}

/// 删除不存在的条目视为成功，其余错误如实返回。
pub fn delete_app_secret(app_id: &str) -> Result<(), String> {
    match entry(app_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("删除 App Secret 失败：{e}")),
    }
}

// ---- 数据源密码：service 同上，用户名 `db:<别名>`。错误信息里只出现别名，不带密码。----

fn db_entry(alias: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &format!("db:{alias}")).map_err(|e| format!("打开凭据库失败：{e}"))
}

pub fn set_db_password(alias: &str, pw: &str) -> Result<(), String> {
    db_entry(alias)?
        .set_password(pw)
        .map_err(|e| format!("保存数据源 {alias} 的密码失败：{e}"))
}

/// Ok(None) = 没有保存过密码（例如免密账号）。
pub fn db_password(alias: &str) -> Result<Option<String>, String> {
    match db_entry(alias)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("读取数据源 {alias} 的密码失败：{e}")),
    }
}

/// 没有条目也算成功。
pub fn delete_db_password(alias: &str) -> Result<(), String> {
    match db_entry(alias)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("删除数据源 {alias} 的密码失败：{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用唯一数据源别名；Drop 时清理。
    struct TestAlias(String);

    impl TestAlias {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(format!("unit_test_db_{}_{nanos}", std::process::id()))
        }
    }

    impl Drop for TestAlias {
        fn drop(&mut self) {
            let _ = delete_db_password(&self.0);
        }
    }

    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn db_password_roundtrip_and_delete() {
        let a = TestAlias::new();
        assert_eq!(db_password(&a.0).unwrap(), None);
        set_db_password(&a.0, "pw-1").unwrap();
        assert_eq!(db_password(&a.0).unwrap().as_deref(), Some("pw-1"));
        set_db_password(&a.0, "pw-2").unwrap();
        assert_eq!(db_password(&a.0).unwrap().as_deref(), Some("pw-2"));
        delete_db_password(&a.0).unwrap();
        assert_eq!(db_password(&a.0).unwrap(), None);
        assert!(delete_db_password(&a.0).is_ok(), "没有条目也算成功");
    }

    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn db_password_does_not_collide_with_app_secret() {
        let a = TestAlias::new();
        set_db_password(&a.0, "db-pw").unwrap();
        assert!(app_secret(&a.0).is_err(), "数据源密码和 App Secret 用不同的用户名前缀");
    }

    /// 测试用唯一 app_id；Drop 时清理，panic 也不会残留凭据。
    struct TestAppId(String);

    impl TestAppId {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(format!("cli_unit_test_{}_{nanos}", std::process::id()))
        }
    }

    impl Drop for TestAppId {
        fn drop(&mut self) {
            let _ = delete_app_secret(&self.0);
        }
    }

    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn secret_roundtrip_in_system_keyring() {
        let id = TestAppId::new();
        assert!(app_secret(&id.0).is_err());
        set_app_secret(&id.0, "s3cret-value").unwrap();
        assert_eq!(app_secret(&id.0).unwrap(), "s3cret-value");
        delete_app_secret(&id.0).unwrap();
        assert!(app_secret(&id.0).is_err());
    }

    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn copy_from_old_service_does_not_overwrite_existing() {
        let id = TestAppId::new();
        let old = format!("dev.beetle.unit-test-old.{}", std::process::id());
        let old_entry = entry_in(&old, &id.0).unwrap();
        assert!(!copy_from_service(&old, &id.0).unwrap(), "旧的没有就什么都不做");
        old_entry.set_password("old-secret").unwrap();
        assert!(copy_from_service(&old, &id.0).unwrap());
        assert_eq!(app_secret(&id.0).unwrap(), "old-secret");
        old_entry.set_password("changed").unwrap();
        assert!(!copy_from_service(&old, &id.0).unwrap(), "新的已有不覆盖");
        assert_eq!(app_secret(&id.0).unwrap(), "old-secret");
        let _ = old_entry.delete_credential();
    }

    #[test]
    #[cfg_attr(not(windows), ignore)]
    fn delete_missing_secret_is_ok() {
        let id = TestAppId::new();
        assert!(delete_app_secret(&id.0).is_ok());
    }
}
