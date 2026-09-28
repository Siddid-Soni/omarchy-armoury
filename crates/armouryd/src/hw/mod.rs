pub mod supergfx;
pub mod sysfs;

use std::future::Future;
use std::time::Duration;

pub const CALL_TIMEOUT: Duration = Duration::from_secs(3);

pub trait Sysfs: Send + Sync {
    fn read(&self, rel: &str) -> Option<String>;
    fn list(&self, rel: &str) -> Vec<String>;
}

#[async_trait::async_trait]
pub trait Gfx: Send + Sync {
    async fn mode(&self) -> anyhow::Result<u32>;
    async fn supported(&self) -> anyhow::Result<Vec<u32>>;
    async fn pending_mode(&self) -> anyhow::Result<u32>;
    async fn power(&self) -> anyhow::Result<u32>;
}

/// Runs `f` up to twice, each attempt bounded by CALL_TIMEOUT (supergfxd can wedge).
pub async fn with_retry<T, F, Fut>(mut f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut last = anyhow::anyhow!("not attempted");
    for _ in 0..2 {
        match tokio::time::timeout(CALL_TIMEOUT, f()).await {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(e)) => last = e,
            Err(_) => last = anyhow::anyhow!("timed out after {:?}", CALL_TIMEOUT),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test(start_paused = true)]
    async fn retry_gives_up_after_two_timeouts() {
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let r: anyhow::Result<u32> = with_retry(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<anyhow::Result<u32>>().await
        })
        .await;
        assert!(r.unwrap_err().to_string().contains("timed out"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(started.elapsed(), CALL_TIMEOUT * 2);
    }

    #[tokio::test]
    async fn retry_recovers_on_second_attempt() {
        let calls = AtomicU32::new(0);
        let r = with_retry(|| async {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 { anyhow::bail!("flaky") } else { Ok(7u32) }
        })
        .await;
        assert_eq!(r.unwrap(), 7);
    }
}
