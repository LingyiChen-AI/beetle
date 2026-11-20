//! SSH 跳板隧道：用系统 ssh 起 `-N -L 127.0.0.1:<本地端口>:<库主机>:<库端口>`，等本地端口能连上再返回守卫；
//! 守卫 Drop 时结束 ssh 进程并回收（查询成功、失败、超时都会走到 Drop）。
//!
//! - 只支持私钥登录（`BatchMode=yes`，不会弹出口令提示卡住）；
//! - Windows 用 `platform::proc::spawn_hidden` 拉起，不弹黑窗；
//! - 本地端口：先绑 `127.0.0.1:0` 拿一个空闲端口再释放，交给 ssh 监听（极小概率被抢，ssh 会因
//!   `ExitOnForwardFailure=yes` 直接退出并报错）。

use crate::platform::proc::{push_tail, spawn_hidden};
use crate::store::datasource::SshTunnel;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 等本地端口可连的最长时间。
const READY_WAIT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
const STDERR_CAP: usize = 8 * 1024;

/// 运行中的隧道。Drop 时结束 ssh。
pub struct Tunnel {
    child: Child,
    pub local_port: u16,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("找不到空闲的本地端口：{e}"))?;
    l.local_addr().map(|a| a.port()).map_err(|e| format!("找不到空闲的本地端口：{e}"))
}

/// IPv6 地址在 -L 参数里要加方括号。
fn bracket(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() }
}

/// 组装 ssh 参数（不含程序名），单独拿出来便于测试。
pub fn ssh_args(t: &SshTunnel, local_port: u16, db_host: &str, db_port: u16) -> Vec<String> {
    vec![
        "-N".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "LogLevel=ERROR".into(),
        "-i".into(),
        t.key_path.clone(),
        "-p".into(),
        t.port.to_string(),
        "-L".into(),
        format!("127.0.0.1:{local_port}:{}:{db_port}", bracket(db_host)),
        format!("{}@{}", t.user, t.host),
    ]
}

/// 阻塞地起隧道（在异步代码里请放进 `spawn_blocking`）。
pub fn open(t: &SshTunnel, db_host: &str, db_port: u16) -> Result<Tunnel, String> {
    let local_port = free_port()?;
    let program = crate::platform::locate::find("ssh").unwrap_or_else(|| "ssh".into());
    let mut cmd = Command::new(program);
    cmd.args(ssh_args(t, local_port, db_host, db_port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = spawn_hidden(&mut cmd).map_err(|e| format!("无法启动 ssh（请确认系统已安装 OpenSSH 客户端）：{e}"))?;
    // 一直读 stderr，免得管道写满卡住 ssh；只留末尾用于报错。
    let tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    if let Some(mut err) = child.stderr.take() {
        let tail = tail.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut t) = tail.lock() {
                    push_tail(&mut t, &buf[..n], STDERR_CAP);
                }
            }
        });
    }
    let mut tunnel = Tunnel { child, local_port };
    let addr = SocketAddr::from(([127, 0, 0, 1], local_port));
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = tunnel.child.try_wait() {
            // 给读线程一点时间把最后的错误读完
            std::thread::sleep(Duration::from_millis(200));
            let msg = tail.lock().map(|t| String::from_utf8_lossy(&t).trim().to_string()).unwrap_or_default();
            let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "无".into());
            return Err(format!(
                "SSH 隧道启动失败（{}@{}:{}，退出码 {code}）：{}",
                t.user,
                t.host,
                t.port,
                if msg.is_empty() { "ssh 没有输出错误信息" } else { msg.as_str() }
            ));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return Ok(tunnel);
        }
        if start.elapsed() >= READY_WAIT {
            // tunnel 在这里 drop，ssh 被结束
            return Err(format!("SSH 隧道 {} 秒内没有建立（{}@{}:{}）", READY_WAIT.as_secs(), t.user, t.host, t.port));
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> SshTunnel {
        SshTunnel { host: "jump.example".into(), port: 2222, user: "ops".into(), key_path: "C:/keys/id".into() }
    }

    #[test]
    fn args_forward_local_port_to_db() {
        let a = ssh_args(&t(), 40001, "10.0.0.8", 3306);
        assert_eq!(a[0], "-N");
        assert!(a.contains(&"BatchMode=yes".to_string()));
        assert!(a.contains(&"ExitOnForwardFailure=yes".to_string()));
        assert!(a.windows(2).any(|w| w[0] == "-i" && w[1] == "C:/keys/id"));
        assert!(a.windows(2).any(|w| w[0] == "-p" && w[1] == "2222"));
        assert!(a.windows(2).any(|w| w[0] == "-L" && w[1] == "127.0.0.1:40001:10.0.0.8:3306"));
        assert_eq!(a.last().unwrap(), "ops@jump.example");
    }

    #[test]
    fn ipv6_db_host_is_bracketed() {
        let a = ssh_args(&t(), 1, "fd00::1", 5432);
        assert!(a.contains(&"127.0.0.1:1:[fd00::1]:5432".to_string()));
    }

    #[test]
    fn free_port_is_nonzero() {
        assert_ne!(free_port().unwrap(), 0);
    }

    #[test]
    fn unreachable_jump_host_fails_and_cleans_up() {
        // 本机没有 ssh 或跳板不可达都应返回错误，且不能挂住超过 READY_WAIT 太久
        let bad = SshTunnel { host: "127.0.0.1".into(), port: 1, user: "nobody".into(), key_path: "Z:/nope/key".into() };
        let start = Instant::now();
        assert!(open(&bad, "127.0.0.1", 3306).is_err());
        assert!(start.elapsed() < READY_WAIT + Duration::from_secs(3));
    }
}
