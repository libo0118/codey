use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use serde_json::{Value, json};
use tokio::sync::{Semaphore, watch};

const MAX_ACTIONS: usize = 4096;
const MAX_PENDING_ACTIONS: usize = 32;
const ACTION_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
enum Outcome {
    Pending,
    Completed(Value),
}

struct Entry {
    digest: [u8; 32],
    result: watch::Receiver<Outcome>,
}

pub(super) struct Ledger {
    entries: HashMap<String, Entry>,
    tails: HashMap<String, watch::Receiver<Outcome>>,
    pending: Arc<Semaphore>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            tails: HashMap::new(),
            pending: Arc::new(Semaphore::new(MAX_PENDING_ACTIONS)),
        }
    }
}

fn uncertain() -> Value {
    json!({"status":"failed","message":super::desktop::UNCERTAIN})
}

struct Completion(watch::Sender<Outcome>);

impl Drop for Completion {
    fn drop(&mut self) {
        if matches!(*self.0.borrow(), Outcome::Pending) {
            self.0.send_replace(Outcome::Completed(uncertain()));
        }
    }
}

pub(super) struct ResultReceiver(watch::Receiver<Outcome>);

impl ResultReceiver {
    pub async fn finish(mut self) -> Value {
        loop {
            if let Outcome::Completed(value) = &*self.0.borrow_and_update() {
                return value.clone();
            }
            if self.0.changed().await.is_err() {
                return uncertain();
            }
        }
    }
}

impl Ledger {
    // Called only under the short ledger lock. Registration fixes per-thread
    // order before any task can wait on desktop I/O.
    pub fn start<F>(
        &mut self,
        id: &str,
        digest: [u8; 32],
        thread: &str,
        operation: F,
    ) -> Result<ResultReceiver, String>
    where
        F: Future<Output = Result<Value, String>> + Send + 'static,
    {
        if let Some(entry) = self.entries.get(id) {
            if entry.digest != digest {
                return Err("同一操作标识不能用于不同内容".into());
            }
            return Ok(ResultReceiver(entry.result.clone()));
        }
        if self.entries.len() >= MAX_ACTIONS {
            return Err("本次远程会话操作记录已满，请在电脑端重启远程服务".into());
        }
        let permit = self
            .pending
            .clone()
            .try_acquire_owned()
            .map_err(|_| "远程操作正在处理中，请稍后重试".to_string())?;
        let (sender, result) = watch::channel(Outcome::Pending);
        let previous = self.tails.insert(thread.to_string(), result.clone());
        self.entries.insert(
            id.to_string(),
            Entry {
                digest,
                result: result.clone(),
            },
        );
        // Disconnecting a caller does not cancel a submitted mutation or free
        // its place in the queue. Duplicates share the final (possibly uncertain) result.
        tokio::spawn(async move {
            let _permit = permit;
            let completion = Completion(sender);
            if let Some(previous) = previous {
                ResultReceiver(previous).finish().await;
            }
            let value = match tokio::time::timeout(ACTION_TIMEOUT, operation).await {
                Ok(result) => {
                    result.unwrap_or_else(|message| json!({"status":"failed","message":message}))
                }
                Err(_) => uncertain(),
            };
            completion.0.send_replace(Outcome::Completed(value));
        });
        Ok(ResultReceiver(result))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn slow_thread_does_not_block_another_and_duplicates_share_completion() {
        let mut ledger = Ledger::default();
        let (release, wait) = tokio::sync::oneshot::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let first = ledger
            .start("one", [1; 32], "a", async move {
                count.fetch_add(1, Ordering::SeqCst);
                wait.await.unwrap();
                Ok(json!(1))
            })
            .unwrap();
        let duplicate = ledger
            .start("one", [1; 32], "a", async { panic!("replayed") })
            .unwrap();
        assert!(
            ledger
                .start("one", [2; 32], "a", async { panic!("changed digest") })
                .is_err()
        );
        let count = calls.clone();
        let second = ledger
            .start("two", [2; 32], "a", async move {
                assert_eq!(count.fetch_add(1, Ordering::SeqCst), 1);
                Ok(json!(2))
            })
            .unwrap();
        let other = ledger
            .start("three", [3; 32], "b", async { Ok(json!(3)) })
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), other.finish())
                .await
                .unwrap(),
            json!(3)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(first); // Cancelling the HTTP waiter must not release the thread queue.
        release.send(()).unwrap();
        assert_eq!(duplicate.finish().await, json!(1));
        assert_eq!(second.finish().await, json!(2));
        assert_eq!(ledger.len(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn uncertain_results_are_retained_and_the_ledger_is_bounded() {
        let mut ledger = Ledger::default();
        let result = ledger
            .start("timeout", [0; 32], "a", std::future::pending())
            .unwrap();
        assert_eq!(result.finish().await, uncertain());
        assert_eq!(
            ledger
                .start("timeout", [0; 32], "a", async { panic!("replayed") })
                .unwrap()
                .finish()
                .await,
            uncertain()
        );
        for n in 1..MAX_ACTIONS {
            ledger
                .start(&n.to_string(), [0; 32], "b", async { Ok(Value::Null) })
                .unwrap()
                .finish()
                .await;
        }
        assert!(
            ledger
                .start("overflow", [0; 32], "c", async { Ok(Value::Null) })
                .is_err()
        );
        assert_eq!(
            ledger
                .start("timeout", [0; 32], "a", async { panic!("replayed") })
                .unwrap()
                .finish()
                .await,
            uncertain()
        );
    }

    #[tokio::test]
    async fn panics_complete_waiters_without_replaying() {
        let mut ledger = Ledger::default();
        let result = ledger
            .start("panic", [0; 32], "a", async { panic!("mock panic") })
            .unwrap();
        assert_eq!(result.finish().await, uncertain());
        assert_eq!(
            ledger
                .start("next", [1; 32], "a", async { Ok(json!("ok")) })
                .unwrap()
                .finish()
                .await,
            json!("ok")
        );
    }

    #[tokio::test]
    async fn pending_actions_are_bounded_without_rejecting_duplicates() {
        let mut ledger = Ledger::default();
        for n in 0..MAX_PENDING_ACTIONS {
            ledger
                .start(&n.to_string(), [0; 32], "a", std::future::pending())
                .unwrap();
        }
        assert!(
            ledger
                .start("overflow", [0; 32], "b", async { Ok(Value::Null) })
                .is_err()
        );
        assert!(
            ledger
                .start("0", [0; 32], "a", async { panic!("replayed") })
                .is_ok()
        );
        assert_eq!(ledger.len(), MAX_PENDING_ACTIONS);
    }
}
