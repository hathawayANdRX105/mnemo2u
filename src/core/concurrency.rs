//! Concurrency control — port of `limit_async_func_call` (`_utils.py:276`).
//!
//! The reference spins on a counter to cap in-flight async calls; we use a
//! tokio semaphore (documented deviation, same observable limit).

use std::future::Future;
use std::sync::Arc;

use tokio::sync::Semaphore;

/// Caps concurrent in-flight calls (reference defaults: 16 per model path,
/// `graphrag.py:117-131`).
#[derive(Clone)]
pub struct Limiter {
    permits: Arc<Semaphore>,
    max: usize,
}

impl Limiter {
    pub fn new(max: usize) -> Self {
        assert!(max > 0, "limiter needs at least one permit");
        Self {
            permits: Arc::new(Semaphore::new(max)),
            max,
        }
    }

    pub fn max(&self) -> usize {
        self.max
    }

    /// Run `fut` once a permit is free; the permit is released on completion.
    pub async fn run<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        let _permit = self
            .permits
            .acquire()
            .await
            .expect("semaphore never closed");
        fut.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn caps_inflight_calls() {
        let limiter = Limiter::new(2);
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let limiter = limiter.clone();
            let current = current.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                limiter
                    .run(async move {
                        let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        current.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
            }));
        }
        for handle in handles {
            handle.await.expect("task completes");
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak {} exceeded the cap",
            peak.load(Ordering::SeqCst)
        );
    }
}
