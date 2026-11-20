//! 旧版 bug-bot（lark-channel-bridge）的系统侧操作：查进程、查 / 停计划任务、读密钥。
//! 平台差异集中在这里（spec §16）；上层只依赖 `LegacySys` / `SecretGetter` 两个 trait，测试可注入。

use super::proc;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// 旧计划任务的默认名。
pub const DEFAULT_TASK: &str = "LarkChannelBridge.Bot.claude";

pub trait LegacySys: Send + Sync {
    fn bridge_running(&self) -> bool;
    /// 计划任务存在且没有被禁用。
    fn task_enabled(&self, task: &str) -> bool;
    /// 停用计划任务并结束 bridge 进程；返回需要告诉用户的警告（空 = 全部成功）。
    fn stop_bridge(&self, task: &str) -> Vec<String>;
}

pub trait SecretGetter: Send + Sync {
    /// 取 App Secret。错误信息里**不得**含任何命令输出（可能带密钥）。
    fn get(&self, app_id: &str) -> Result<String, String>;
}

#[cfg_attr(not(windows), allow(dead_code))]
fn safe_task_name(task: &str) -> bool {
    !task.is_empty() && task.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub struct SystemLegacy;

/// 运行一段 PowerShell，返回 stdout。
#[cfg(windows)]
fn powershell(script: &str) -> Result<String, String> {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行 PowerShell：{e}"))?;
    let out = proc::wait_output_timeout(child, Duration::from_secs(20))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("未知错误").trim().to_string())
    }
}

// 进程名片段拆开写，避免 PowerShell 自己的命令行匹配上自己。
#[cfg(windows)]
const PS_BRIDGE_FILTER: &str = "$n = 'lark-channel-' + 'bridge'; \
    Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and $_.CommandLine -like ('*' + $n + '*') -and $_.ProcessId -ne $PID }";

#[cfg(windows)]
impl LegacySys for SystemLegacy {
    fn bridge_running(&self) -> bool {
        let script = format!("({PS_BRIDGE_FILTER} | Measure-Object).Count");
        powershell(&script).ok().and_then(|s| s.parse::<u32>().ok()).is_some_and(|n| n > 0)
    }

    fn task_enabled(&self, task: &str) -> bool {
        if !safe_task_name(task) {
            return false;
        }
        let script = format!("try {{ (Get-ScheduledTask -TaskName '{task}' -ErrorAction Stop).State }} catch {{ 'Missing' }}");
        powershell(&script).is_ok_and(|s| s != "Missing" && s != "Disabled" && !s.is_empty())
    }

    fn stop_bridge(&self, task: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        if !safe_task_name(task) {
            warnings.push(format!("旧计划任务名不合法，未处理：{task}"));
        } else if self.task_enabled(task) {
            let script = format!("Disable-ScheduledTask -TaskName '{task}' -ErrorAction Stop | Out-Null; Stop-ScheduledTask -TaskName '{task}' -ErrorAction SilentlyContinue");
            if let Err(e) = powershell(&script) {
                warnings.push(format!("停用计划任务 {task} 失败（可能需要管理员权限，请以管理员身份在任务计划程序里手动禁用）：{e}"));
            }
        }
        let kill = format!("{PS_BRIDGE_FILTER} | ForEach-Object {{ taskkill /T /F /PID $_.ProcessId | Out-Null }}");
        if let Err(e) = powershell(&kill) {
            warnings.push(format!("结束旧 bridge 进程失败：{e}"));
        }
        if self.bridge_running() {
            warnings.push("旧 bridge 进程仍在运行，请在任务管理器里手动结束 node.exe（命令行含 lark-channel-bridge）".into());
        }
        warnings
    }
}

#[cfg(not(windows))]
impl LegacySys for SystemLegacy {
    fn bridge_running(&self) -> bool {
        Command::new("pgrep").args(["-f", "lark-channel-bridge"]).output().is_ok_and(|o| o.status.success())
    }

    fn task_enabled(&self, _task: &str) -> bool {
        false
    }

    fn stop_bridge(&self, _task: &str) -> Vec<String> {
        let ok = Command::new("pkill").args(["-f", "lark-channel-bridge"]).status().is_ok();
        let mut w = vec!["macOS 上的 launchd 服务需要手动停用（launchctl unload 对应的 plist）".to_string()];
        if !ok {
            w.push("结束旧 bridge 进程失败，请手动处理".into());
        }
        w
    }
}

/// 经旧 bridge 的 `secrets-getter.cmd` 取密钥（协议已实测）：
/// `<getter> app-<app_id>`，stdin 写 `{"protocolVersion":1,"provider":"bridge","ids":["app-<app_id>"]}`，
/// stdout 返回 `{"protocolVersion":1,"values":{"app-<app_id>":"<secret>"}}`。
pub struct BridgeSecretGetter {
    pub script: PathBuf,
}

/// 解析 getter 的 stdout。错误信息不含输出内容。
pub fn parse_getter_output(stdout: &str, key: &str) -> Result<String, String> {
    let start = stdout.find('{').ok_or("旧 bridge 没有返回密钥")?;
    let end = stdout.rfind('}').ok_or("旧 bridge 没有返回密钥")?;
    let v: serde_json::Value = serde_json::from_str(&stdout[start..=end]).map_err(|_| "旧 bridge 返回的格式无法解析".to_string())?;
    v.pointer(&format!("/values/{key}"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "旧 bridge 里没有这个应用的密钥".to_string())
}

impl SecretGetter for BridgeSecretGetter {
    fn get(&self, app_id: &str) -> Result<String, String> {
        use std::io::Write;
        if !self.script.is_file() {
            return Err(format!("找不到 {}", self.script.display()));
        }
        let key = format!("app-{app_id}");
        let request = serde_json::json!({"protocolVersion": 1, "provider": "bridge", "ids": [key]}).to_string();
        let mut cmd = Command::new(&self.script);
        cmd.arg(&key).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = proc::spawn_hidden(&mut cmd).map_err(|e| format!("无法运行密钥读取脚本：{e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(request.as_bytes());
        }
        let out = proc::wait_output_timeout(child, Duration::from_secs(20))?;
        if !out.status.success() {
            return Err(format!("密钥读取脚本退出码 {:?}", out.status.code()));
        }
        parse_getter_output(&String::from_utf8_lossy(&out.stdout), &key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_getter_output_and_errors_never_echo_it() {
        let out = r#"{"protocolVersion":1,"values":{"app-cli_x":"s3cr3t"}}"#;
        assert_eq!(parse_getter_output(out, "app-cli_x").unwrap(), "s3cr3t");
        assert_eq!(parse_getter_output(&format!("提示\n{out}\n"), "app-cli_x").unwrap(), "s3cr3t");
        let missing = parse_getter_output(out, "app-other").unwrap_err();
        assert!(!missing.contains("s3cr3t"));
        let bad = parse_getter_output("{oops s3cr3t}", "app-cli_x").unwrap_err();
        assert!(!bad.contains("s3cr3t"));
        assert!(parse_getter_output("", "k").is_err());
    }

    #[test]
    fn task_names_are_restricted() {
        assert!(safe_task_name(DEFAULT_TASK));
        assert!(!safe_task_name("a'; calc"));
        assert!(!safe_task_name(""));
    }
}
