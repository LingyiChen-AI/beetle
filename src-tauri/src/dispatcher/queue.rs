//! 运行队列：同一 `chat_id` 严格串行，整体并发不超过上限；不同群并行。
//! 队列本身不关心怎么运行任务，由调用方给一个执行器（Task 11 里接 `run_once`）。

use super::Job;
use futures_util::future::BoxFuture;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// 执行器：拿到任务和取消令牌，返回一个跑完即结束的 future。
pub type Executor = Arc<dyn Fn(Job, CancellationToken) -> BoxFuture<'static, ()> + Send + Sync>;

struct Pending {
    id: String,
    job: Job,
    token: CancellationToken,
    done: oneshot::Sender<()>,
}

#[derive(Default)]
struct State {
    pending: VecDeque<Pending>,
    running_chats: HashSet<String>,
    running: HashMap<String, CancellationToken>,
}

struct Inner {
    concurrency: usize,
    executor: Executor,
    state: Mutex<State>,
}

#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

/// 提交后的句柄。`id` 用于取消；`finished()` 等任务跑完。
pub struct JobHandle {
    pub id: String,
    done: oneshot::Receiver<()>,
}

impl JobHandle {
    /// 任务跑完返回 true；排队中被取消（从未执行）返回 false。
    pub async fn finished(self) -> bool {
        self.done.await.is_ok()
    }
}

/// 任务结束（含 executor panic）时释放槽位并继续调度。
struct Release {
    inner: Arc<Inner>,
    id: String,
    chat_id: String,
}

impl Drop for Release {
    fn drop(&mut self) {
        {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            st.running.remove(&self.id);
            st.running_chats.remove(&self.chat_id);
        }
        pump(&self.inner);
    }
}

fn pump(inner: &Arc<Inner>) {
    loop {
        let next = {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.running.len() >= inner.concurrency {
                return;
            }
            let Some(pos) = st
                .pending
                .iter()
                .position(|p| !st.running_chats.contains(&p.job.msg.chat_id))
            else {
                return;
            };
            let p = st.pending.remove(pos).expect("position is valid");
            st.running_chats.insert(p.job.msg.chat_id.clone());
            st.running.insert(p.id.clone(), p.token.clone());
            p
        };
        let inner2 = inner.clone();
        tokio::spawn(async move {
            let Pending { id, job, token, done } = next;
            let _release = Release { inner: inner2.clone(), id, chat_id: job.msg.chat_id.clone() };
            (inner2.executor)(job, token).await;
            let _ = done.send(());
        });
    }
}

impl Queue {
    /// `concurrency` 至少按 1 处理。必须在 tokio 运行时内使用。
    pub fn new(concurrency: u32, executor: Executor) -> Self {
        Queue {
            inner: Arc::new(Inner {
                concurrency: concurrency.max(1) as usize,
                executor,
                state: Mutex::new(State::default()),
            }),
        }
    }

    pub fn submit(&self, job: Job) -> JobHandle {
        let id = job.run_id.clone();
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            st.pending.push_back(Pending {
                id: id.clone(),
                job,
                token: CancellationToken::new(),
                done: tx,
            });
        }
        pump(&self.inner);
        JobHandle { id, done: rx }
    }

    /// 取消：运行中的任务取消其令牌；排队中的直接移除。返回是否找到该任务。
    pub fn cancel(&self, id: &str) -> bool {
        let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(token) = st.running.get(id) {
            token.cancel();
            return true;
        }
        if let Some(pos) = st.pending.iter().position(|p| p.id == id) {
            st.pending.remove(pos);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feishu::transport::IncomingMessage;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Semaphore, mpsc};

    fn job(chat: &str, mid: &str) -> Job {
        Job {
            run_id: format!("run_{mid}"),
            bot_id: "b1".into(),
            msg: IncomingMessage {
                message_id: mid.into(),
                chat_id: chat.into(),
                sender_open_id: "ou_x".into(),
                sender_union_id: None,
                text: "t".into(),
                is_group: true,
                mentions_bot: true,
                message_type: "text".into(),
                parent_id: None,
                root_id: None,
                create_time_ms: 0,
            },
            resume: None,
            needs_onboard: false,
            pr: None,
        }
    }

    async fn settle() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn same_group_runs_serially() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let gate = Arc::new(Semaphore::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel::<String>();
        let ev = events.clone();
        let g = gate.clone();
        let exec: Executor = Arc::new(move |j: Job, _t| {
            let (ev, g, tx) = (ev.clone(), g.clone(), started_tx.clone());
            Box::pin(async move {
                let m = j.msg.message_id.clone();
                ev.lock().unwrap().push(format!("start {m}"));
                let _ = tx.send(m.clone());
                g.acquire().await.unwrap().forget();
                ev.lock().unwrap().push(format!("end {m}"));
            })
        });
        // 并发上限 2，仍然要求同群串行
        let q = Queue::new(2, exec);
        let h1 = q.submit(job("oc_a", "m1"));
        let h2 = q.submit(job("oc_a", "m2"));
        assert_eq!(started_rx.recv().await.unwrap(), "m1");
        settle().await;
        assert_eq!(*events.lock().unwrap(), vec!["start m1"]);
        gate.add_permits(2);
        assert!(h1.finished().await);
        assert!(h2.finished().await);
        assert_eq!(
            *events.lock().unwrap(),
            vec!["start m1", "end m1", "start m2", "end m2"]
        );
    }

    #[tokio::test]
    async fn concurrency_limit_respected_across_groups() {
        let cur = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel::<()>();
        let (c, m, g) = (cur.clone(), max.clone(), gate.clone());
        let exec: Executor = Arc::new(move |_j: Job, _t| {
            let (c, m, g, tx) = (c.clone(), m.clone(), g.clone(), started_tx.clone());
            Box::pin(async move {
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(n, Ordering::SeqCst);
                let _ = tx.send(());
                g.acquire().await.unwrap().forget();
                c.fetch_sub(1, Ordering::SeqCst);
            })
        });
        let q = Queue::new(2, exec);
        let hs: Vec<_> = ["oc_a", "oc_b", "oc_c"]
            .iter()
            .enumerate()
            .map(|(i, c)| q.submit(job(c, &format!("m{i}"))))
            .collect();
        started_rx.recv().await.unwrap();
        started_rx.recv().await.unwrap();
        settle().await;
        assert_eq!(cur.load(Ordering::SeqCst), 2, "第三个必须排队等待");
        gate.add_permits(3);
        for h in hs {
            assert!(h.finished().await);
        }
        assert_eq!(max.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancel_running_job_cancels_token() {
        let (started_tx, mut started_rx) = mpsc::unbounded_channel::<()>();
        let exec: Executor = Arc::new(move |_j: Job, t: CancellationToken| {
            let tx = started_tx.clone();
            Box::pin(async move {
                let _ = tx.send(());
                t.cancelled().await;
            })
        });
        let q = Queue::new(1, exec);
        let h = q.submit(job("oc_a", "m1"));
        assert_eq!(h.id, "run_m1");
        started_rx.recv().await.unwrap();
        assert!(q.cancel("run_m1"));
        assert!(h.finished().await);
    }

    #[tokio::test]
    async fn cancel_queued_job_drops_it_without_running() {
        let ran = Arc::new(Mutex::new(Vec::<String>::new()));
        let gate = Arc::new(Semaphore::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel::<()>();
        let (r, g) = (ran.clone(), gate.clone());
        let exec: Executor = Arc::new(move |j: Job, _t| {
            let (r, g, tx) = (r.clone(), g.clone(), started_tx.clone());
            Box::pin(async move {
                r.lock().unwrap().push(j.msg.message_id.clone());
                let _ = tx.send(());
                g.acquire().await.unwrap().forget();
            })
        });
        let q = Queue::new(1, exec);
        let h1 = q.submit(job("oc_a", "m1"));
        let h2 = q.submit(job("oc_a", "m2"));
        let h3 = q.submit(job("oc_a", "m3"));
        started_rx.recv().await.unwrap();
        assert!(q.cancel(&h2.id));
        assert!(!q.cancel(&h2.id), "已移除的任务再取消返回 false");
        gate.add_permits(3);
        assert!(h1.finished().await);
        assert!(!h2.finished().await, "排队中被取消的任务从未执行");
        assert!(h3.finished().await);
        assert_eq!(*ran.lock().unwrap(), vec!["m1", "m3"]);
    }

    #[tokio::test]
    async fn panicking_executor_releases_slot() {
        let exec: Executor = Arc::new(move |j: Job, _t| {
            Box::pin(async move {
                if j.msg.message_id == "m1" {
                    panic!("boom");
                }
            })
        });
        let q = Queue::new(1, exec);
        let h1 = q.submit(job("oc_a", "m1"));
        let h2 = q.submit(job("oc_a", "m2"));
        assert!(!h1.finished().await);
        assert!(h2.finished().await);
    }
}
