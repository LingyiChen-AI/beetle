//! 子进程监管：隐藏窗口拉起 Agent CLI，按行流式读 stdout，超时 / 取消时结束整棵进程树。
//!
//! - Windows：子进程放进 Job Object（`KILL_ON_JOB_CLOSE`），`kill_tree` 与句柄 drop 都会杀掉整个 Job
//!   （含 python / node / git 孙进程）；加入 Job 失败时退回 `taskkill /T /F`。
//! - 非 Windows：子进程自成进程组，`kill -KILL -- -pgid`（macOS 在第 4 阶段才验证，这里只保证能编译）。
//! - 已知限制：`CreateProcess` 到 `AssignProcessToJobObject` 之间有极短的竞态窗口，此刻已派生的孙进程不在 Job 内；
//!   孙进程主动 breakaway 同样不受 Job 约束。因此行流的结束不依赖「所有孙进程都死了」，见下条。
//! - stdout：每行一个 `Vec<u8>`（不含 `\n` / `\r\n`），经 tokio mpsc 通道送出；通道在 stdout EOF、`kill_tree` 被调用、或主进程退出 2 秒后仍未 EOF 时关闭。
//!   单行最长 `MAX_LINE_BYTES`（1 MiB）：超长行只保留前 1 MiB、其余丢弃，仍作为一行送出（内容是被截断的），
//!   内存占用有上界。
//! - stderr：只保留最后 `STDERR_TAIL_BYTES`（64 KiB）。
//! - stdin：写入 `spec.stdin` 后关闭（在独立线程里写，大 prompt 不会和输出互相卡死）。

use crate::agent::LaunchSpec;
use std::io::{Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub const STDERR_TAIL_BYTES: usize = 64 * 1024;
/// 进程退出后等 stderr 读完的最长时间（孙进程可能还攥着管道）。
const STDERR_JOIN_WAIT: Duration = Duration::from_secs(2);
/// 主进程退出后，stdout 持续无新数据的宽限期；超过就强制结束行流（孙进程可能还攥着管道）。
/// 只在缓冲已排空、读线程也没有新行时才计时，消费者慢不会消耗它。
const EXIT_GRACE: Duration = Duration::from_secs(2);
const FORWARD_POLL: Duration = Duration::from_millis(50);
/// 行通道容量：消费者慢时读线程阻塞，管道反压给子进程，不会无限堆内存。
const LINE_CHANNEL_CAP: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct ExitInfo {
    /// 被杀时可能没有退出码（非 Windows 的信号终止）。
    pub code: Option<i32>,
    pub stderr_tail: String,
}

/// 隐藏窗口拉起子进程（Windows `CREATE_NO_WINDOW`；Unix 自成进程组）。stdio 由调用方配置。
pub fn spawn_hidden(cmd: &mut Command) -> std::io::Result<Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

/// 结束 pid 为根的进程树（不依赖 Job，用于兜底和 `version` 超时）。
fn kill_pid_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// 带超时地等一个已拉起的子进程，同时读完 stdout / stderr；超时则杀进程树并报错。
pub fn wait_output_timeout(mut child: Child, timeout: Duration) -> Result<Output, String> {
    fn drain<R: Read + Send + 'static>(mut r: R) -> std::sync::mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        rx
    }
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                kill_pid_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("超过 {} 秒未结束，已终止", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                kill_pid_tree(child.id());
                return Err(e.to_string());
            }
        }
    };
    let join = |rx: Option<std::sync::mpsc::Receiver<Vec<u8>>>| {
        rx.and_then(|r| r.recv_timeout(STDERR_JOIN_WAIT).ok()).unwrap_or_default()
    };
    Ok(Output { status, stdout: join(out), stderr: join(err) })
}

/// 按 `\n` 切行读 `r`，每行（去掉 `\n` 与 `\r`）交给 `f`。单行最多保留 `cap` 字节，超出部分丢弃。
pub fn read_capped_lines<R: Read>(r: R, cap: usize, mut f: impl FnMut(Vec<u8>)) {
    let mut r = std::io::BufReader::with_capacity(64 * 1024, r);
    let mut line: Vec<u8> = Vec::new();
    let mut has_data = false;
    loop {
        let chunk = match std::io::BufRead::fill_buf(&mut r) {
            Ok([]) => break,
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let (take, found_nl) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        has_data = true;
        let body = if found_nl { &chunk[..take - 1] } else { &chunk[..take] };
        let room = cap.saturating_sub(line.len());
        line.extend_from_slice(&body[..body.len().min(room)]);
        std::io::BufRead::consume(&mut r, take);
        if found_nl {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            f(std::mem::take(&mut line));
            has_data = false;
        }
    }
    if has_data {
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        f(line);
    }
}

/// 把 `data` 追加到 `buf`，只保留最后 `cap` 字节。
pub fn push_tail(buf: &mut Vec<u8>, data: &[u8], cap: usize) {
    if data.len() >= cap {
        buf.clear();
        buf.extend_from_slice(&data[data.len() - cap..]);
        return;
    }
    buf.extend_from_slice(data);
    if buf.len() > cap {
        buf.drain(..buf.len() - cap);
    }
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);
// Job 句柄只是内核对象句柄，跨线程使用安全。
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    /// 新建带 `KILL_ON_JOB_CLOSE` 的 Job 并把进程放进去；任何一步失败返回 None。
    fn create_with(child: &Child) -> Option<Job> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let job = Job(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(h, child.as_raw_handle() as _) == 0 {
                return None; // drop 会关闭句柄
            }
            Some(job)
        }
    }

    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE：关闭最后一个句柄时杀掉 Job 内所有进程。
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// 一个受监管的子进程。drop 时（Windows）整棵树被结束。
pub struct ChildHandle {
    pid: u32,
    #[cfg(windows)]
    job: Option<Job>,
    lines: Option<mpsc::Receiver<Vec<u8>>>,
    done: oneshot::Receiver<ExitInfo>,
    /// kill_tree 被调用过：转发层据此结束行流。
    killed: Arc<AtomicBool>,
    /// 是否真的去结束 OS 进程（仅测试可关，用来构造「杀不到孙进程」的场景）。
    os_kill: bool,
}

impl ChildHandle {
    /// 取走 stdout 行流（只能取一次，之后返回已关闭的通道）。取走后 `kill_tree(&self)` 仍可在消费者等待时调用。
    pub fn lines(&mut self) -> mpsc::Receiver<Vec<u8>> {
        self.lines.take().unwrap_or_else(|| mpsc::channel(1).1)
    }

    /// 结束进程树。可重复调用；之后行流会结束。
    pub fn kill_tree(&self) {
        self.killed.store(true, Ordering::SeqCst);
        if !self.os_kill {
            return;
        }
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
            return;
        }
        kill_pid_tree(self.pid);
    }

    /// 等进程退出，取退出码和 stderr 末尾。
    pub async fn wait(self) -> ExitInfo {
        let ChildHandle { done, .. } = self;
        done.await.unwrap_or(ExitInfo { code: None, stderr_tail: String::new() })
        // 其余字段（含 Job）在此 drop：进程已退出，顺带清掉残留孙进程。
    }
}

/// 按 `spec` 拉起子进程。
pub fn spawn(spec: &LaunchSpec) -> Result<ChildHandle, String> {
    spawn_inner(spec, true, true)
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn spawn_inner(spec: &LaunchSpec, use_job: bool, os_kill: bool) -> Result<ChildHandle, String> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child =
        spawn_hidden(&mut cmd).map_err(|e| format!("无法启动 {}：{e}", spec.program.display()))?;
    let pid = child.id();
    #[cfg(windows)]
    let job = if use_job { Job::create_with(&child) } else { None };

    // stdin：写完即关。
    if let Some(mut stdin) = child.stdin.take() {
        let data = spec.stdin.clone().into_bytes();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
            let _ = stdin.flush();
            // drop 关闭管道
        });
    }

    // stdout：读线程 -> 内部通道 -> 转发线程 -> 消费者通道。
    // 读线程阻塞在 read 上，只有管道所有写端关闭才返回；若有孙进程逃出 Job 仍攥着管道，它会一直阻塞
    // （后台泄漏一个线程，可接受）。所以消费者的行流不直接由它驱动，而由转发线程在
    // (a) 读线程 EOF、(b) kill_tree 被调用、(c) 主进程退出超过宽限期仍未 EOF 任一成立时结束。
    // 转发线程结束后内部通道的接收端被丢弃，读线程的发送失败后只读不发（也不会无限 send 阻塞）。
    let (tx, rx) = mpsc::channel::<Vec<u8>>(LINE_CHANNEL_CAP);
    let killed = Arc::new(AtomicBool::new(false));
    let exited_at = Arc::new(Mutex::new(None::<Instant>));
    let (raw_tx, raw_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(LINE_CHANNEL_CAP);
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut open = true;
            read_capped_lines(stdout, MAX_LINE_BYTES, |l| {
                if open && raw_tx.send(l).is_err() {
                    open = false;
                }
            });
        });
    } else {
        drop(raw_tx);
    }
    {
        let killed = killed.clone();
        let exited_at = exited_at.clone();
        std::thread::spawn(move || {
            let mut idle_since: Option<Instant> = None;
            loop {
                if killed.load(Ordering::SeqCst) {
                    break;
                }
                match raw_rx.recv_timeout(FORWARD_POLL) {
                    Ok(line) => {
                        idle_since = None;
                        if tx.blocking_send(line).is_err() {
                            break; // 消费者已丢弃接收端
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // 缓冲暂时为空：主进程已退出且持续空闲超过宽限期才放弃等 EOF。
                        if exited_at.lock().unwrap().is_some() {
                            let since = *idle_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= EXIT_GRACE {
                                while let Ok(line) = raw_rx.try_recv() {
                                    if tx.blocking_send(line).is_err() {
                                        break;
                                    }
                                }
                                break;
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // tx 在此 drop：消费者的 recv 返回 None。
        });
    }

    // stderr：滚动保留末尾。
    let tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (err_done_tx, err_done_rx) = std::sync::mpsc::channel::<()>();
    if let Some(mut stderr) = child.stderr.take() {
        let tail = tail.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                push_tail(&mut tail.lock().unwrap(), &buf[..n], STDERR_TAIL_BYTES);
            }
            let _ = err_done_tx.send(());
        });
    } else {
        let _ = err_done_tx.send(());
    }

    // 等待线程：回收子进程，给 stderr 一点时间收尾。
    let (done_tx, done_rx) = oneshot::channel();
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|s| s.code());
        *exited_at.lock().unwrap() = Some(Instant::now());
        let _ = err_done_rx.recv_timeout(STDERR_JOIN_WAIT);
        let stderr_tail = String::from_utf8_lossy(&tail.lock().unwrap()).into_owned();
        let _ = done_tx.send(ExitInfo { code, stderr_tail });
    });

    Ok(ChildHandle {
        pid,
        #[cfg(windows)]
        job,
        lines: Some(rx),
        done: done_rx,
        killed,
        os_kill,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn ps_spec(script: &str, stdin: &str) -> LaunchSpec {
        LaunchSpec {
            program: PathBuf::from("powershell.exe"),
            args: vec!["-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), script.into()],
            cwd: std::env::temp_dir(),
            env: vec![],
            stdin: stdin.into(),
            last_message_file: None,
        }
    }

    async fn collect(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> Vec<String> {
        let mut v = Vec::new();
        while let Some(l) = rx.recv().await {
            v.push(String::from_utf8_lossy(&l).into_owned());
        }
        v
    }

    fn pid_alive(pid: u32) -> bool {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn streams_stdout_lines_in_order() {
        let mut h = spawn(&ps_spec("1..5 | ForEach-Object { Write-Output \"line$_\" }", "")).unwrap();
        let mut rx = h.lines();
        let got = collect(&mut rx).await;
        assert_eq!(got, ["line1", "line2", "line3", "line4", "line5"]);
        let info = h.wait().await;
        assert_eq!(info.code, Some(0));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn writes_stdin_then_closes_it() {
        let mut h = spawn(&ps_spec("$t = [Console]::In.ReadToEnd(); Write-Output \"got:$($t.Trim())\"", "你好 prompt")).unwrap();
        let mut rx = h.lines();
        let got = collect(&mut rx).await;
        assert_eq!(got, ["got:你好 prompt"]);
        h.wait().await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn applies_env_cwd_and_reports_exit_code_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = ps_spec(
            "Write-Output $env:BP_TEST; Write-Output (Get-Location).Path; [Console]::Error.WriteLine('boom'); exit 3",
            "",
        );
        spec.cwd = dir.path().to_path_buf();
        spec.env = vec![("BP_TEST".into(), "v1".into())];
        let mut h = spawn(&spec).unwrap();
        let mut rx = h.lines();
        let got = collect(&mut rx).await;
        assert_eq!(got[0], "v1");
        assert!(got[1].to_lowercase().ends_with(&dir.path().file_name().unwrap().to_string_lossy().to_lowercase()));
        let info = h.wait().await;
        assert_eq!(info.code, Some(3));
        assert!(info.stderr_tail.contains("boom"), "{:?}", info.stderr_tail);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn kill_tree_ends_grandchildren() {
        let script = "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-Command','Start-Sleep 600'; \
                      Write-Output $p.Id; Start-Sleep 600";
        let mut h = spawn(&ps_spec(script, "")).unwrap();
        let mut rx = h.lines();
        let first = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("等孙进程 pid 超时")
            .expect("没有输出 pid");
        let gpid: u32 = String::from_utf8_lossy(&first).trim().parse().unwrap();
        assert!(pid_alive(gpid), "孙进程应已启动");
        // 清理保险：即使断言失败，句柄 drop 时 Job 也会杀掉整棵树。
        let start = Instant::now();
        h.kill_tree();
        // 消费者正在等待时被杀：流必须结束而不是挂住。
        let end = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("kill 后行流没有结束");
        assert!(end.is_none());
        assert!(!pid_alive(gpid), "孙进程仍然存在");
        assert!(start.elapsed() < Duration::from_secs(3), "耗时 {:?}", start.elapsed());
        let info = h.wait().await;
        assert_ne!(info.code, Some(0));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn kill_tree_while_consumer_is_awaiting_in_select() {
        let mut h = spawn(&ps_spec("Start-Sleep 600", "")).unwrap();
        let mut rx = h.lines();
        let r = tokio::select! {
            l = rx.recv() => l,
            _ = tokio::time::sleep(Duration::from_millis(300)) => { h.kill_tree(); rx.recv().await }
        };
        assert!(r.is_none());
        h.wait().await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn dropping_handle_kills_the_tree() {
        let mut h = spawn(&ps_spec("Write-Output 'up'; Start-Sleep 600", "")).unwrap();
        let mut rx = h.lines();
        assert_eq!(rx.recv().await.unwrap(), b"up");
        drop(h);
        let end = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("drop 后行流没有结束");
        assert!(end.is_none());
    }

    struct PidGuard(Vec<u32>);
    impl Drop for PidGuard {
        fn drop(&mut self) {
            for p in &self.0 {
                kill_pid_tree(*p);
            }
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn stream_ends_after_kill_even_if_grandchild_holds_pipe() {
        // 不加入 Job、kill_tree 也不去杀 OS 进程：主进程和孙进程都还活着、孙进程攥着 stdout 管道，
        // 行流仍必须在 kill_tree 后结束。
        let script = "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-Command','Start-Sleep 600';                       Write-Output $p.Id; Start-Sleep 600";
        let mut h = spawn_inner(&ps_spec(script, ""), false, false).unwrap();
        let mut guard = PidGuard(vec![h.pid]);
        let mut rx = h.lines();
        let first = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.unwrap().unwrap();
        let gpid: u32 = String::from_utf8_lossy(&first).trim().parse().unwrap();
        guard.0.push(gpid);
        assert!(pid_alive(gpid));
        h.kill_tree();
        let end = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.expect("kill 后行流挂住");
        assert!(end.is_none());
        assert!(pid_alive(gpid), "测试前提：孙进程此时仍持有管道");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn stream_ends_after_child_exit_with_grace() {
        // 主进程很快退出，孙进程（在 Job 内但没被杀）攥着管道：宽限期后行流结束。
        let script = "$p = Start-Process -NoNewWindow -PassThru powershell -ArgumentList '-Command','Start-Sleep 600';                       Write-Output $p.Id";
        let mut h = spawn(&ps_spec(script, "")).unwrap();
        let mut guard = PidGuard(vec![h.pid]);
        let mut rx = h.lines();
        let first = tokio::time::timeout(Duration::from_secs(20), rx.recv()).await.unwrap().unwrap();
        let gpid: u32 = String::from_utf8_lossy(&first).trim().parse().unwrap();
        guard.0.push(gpid);
        let start = Instant::now();
        let end = tokio::time::timeout(Duration::from_secs(6), rx.recv()).await.expect("主进程退出后行流挂住");
        assert!(end.is_none());
        assert!(start.elapsed() >= Duration::from_secs(1), "应等了宽限期，实际 {:?}", start.elapsed());
        assert!(pid_alive(gpid), "测试前提：孙进程此时仍持有管道");
        let info = h.wait().await;
        assert_eq!(info.code, Some(0));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn slow_consumer_receives_all_lines_after_normal_exit() {
        let mut h = spawn(&ps_spec("1..599 | ForEach-Object { Write-Output \"row$_\" }; Write-Output 'RESULT-MARKER'", "")).unwrap();
        let mut rx = h.lines();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let got = collect(&mut rx).await;
        assert_eq!(got.len(), 600, "丢行了");
        assert_eq!(got[0], "row1");
        assert_eq!(got.last().unwrap(), "RESULT-MARKER");
        assert_eq!(h.wait().await.code, Some(0));
    }

    #[test]
    fn spawn_reports_missing_program() {
        let mut spec = ps_spec("", "");
        spec.program = PathBuf::from("Z:/nope/missing.exe");
        let e = spawn(&spec).err().expect("应失败");
        assert!(e.contains("missing.exe"), "{e}");
    }

    #[test]
    fn overlong_line_is_capped() {
        let mut data = vec![b'a'; 3 * 1024 * 1024];
        data.extend_from_slice(b"\nnext\r\nlast");
        let mut got: Vec<Vec<u8>> = Vec::new();
        read_capped_lines(Cursor::new(data), MAX_LINE_BYTES, |l| got.push(l));
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].len(), MAX_LINE_BYTES, "超长行截断到上限，其余丢弃");
        assert_eq!(got[1], b"next");
        assert_eq!(got[2], b"last");
    }

    #[test]
    fn stderr_tail_keeps_only_last_bytes() {
        let mut buf = Vec::new();
        push_tail(&mut buf, &vec![b'x'; 100], 64);
        push_tail(&mut buf, b"END", 64);
        assert_eq!(buf.len(), 64);
        assert!(buf.ends_with(b"END"));
    }
}
