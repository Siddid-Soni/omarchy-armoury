pub mod asusd;
pub mod fake;
pub mod nvidia;
pub mod services;
pub mod supergfx;
pub mod sysfs;

use crate::features::fan::RawCurve;
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

/// The NVIDIA dGPU. Implementations must not touch the device unless it is already awake.
pub trait Nvidia: Send + Sync {
    /// PCI runtime PM state is "active"; None when there is no NVIDIA dGPU.
    fn dgpu_active(&self) -> Option<bool>;
    /// NVML readings; only call while dgpu_active() is Some(true).
    fn status(&self) -> Option<armoury_proto::NvStatus>;
    /// Processes holding /dev/nvidia*; only call while the dGPU is awake.
    fn users(&self) -> Vec<armoury_proto::GpuUser>;
}

pub const GHELPER_UNIT: &str = "app-ghelper@autostart.service";

#[async_trait::async_trait]
pub trait Services: Send + Sync {
    async fn run(&self, argv: &[&str]) -> anyhow::Result<()>;
    async fn is_running(&self, process: &str) -> bool;
    async fn unit_active(&self, unit: &str, user: bool) -> bool;
}

#[async_trait::async_trait]
pub trait Asusd: Send + Sync {
    async fn set_profile(&self, p: u32) -> anyhow::Result<()>;
    async fn next_profile(&self) -> anyhow::Result<()>;
    async fn fan_curves(&self, profile: u32) -> anyhow::Result<Vec<RawCurve>>;
    async fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> anyhow::Result<()>;
    async fn reset_fan_curves(&self, profile: u32) -> anyhow::Result<()>;
    async fn set_fan_curves_enabled(&self, profile: u32, enabled: bool) -> anyhow::Result<()>;
    async fn armoury_range(&self, attr: &str) -> anyhow::Result<(i32, i32)>;
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl<T: Services + ?Sized> Services for std::sync::Arc<T> {
    async fn run(&self, argv: &[&str]) -> anyhow::Result<()> { (**self).run(argv).await }
    async fn is_running(&self, process: &str) -> bool { (**self).is_running(process).await }
    async fn unit_active(&self, unit: &str, user: bool) -> bool { (**self).unit_active(unit, user).await }
}

impl<T: Sysfs + ?Sized> Sysfs for std::sync::Arc<T> {
    fn read(&self, rel: &str) -> Option<String> { (**self).read(rel) }
    fn list(&self, rel: &str) -> Vec<String> { (**self).list(rel) }
}

#[async_trait::async_trait]
impl<T: Asusd + ?Sized> Asusd for std::sync::Arc<T> {
    async fn set_profile(&self, p: u32) -> anyhow::Result<()> { (**self).set_profile(p).await }
    async fn next_profile(&self) -> anyhow::Result<()> { (**self).next_profile().await }
    async fn fan_curves(&self, profile: u32) -> anyhow::Result<Vec<RawCurve>> { (**self).fan_curves(profile).await }
    async fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> anyhow::Result<()> { (**self).set_fan_curve(profile, curve).await }
    async fn reset_fan_curves(&self, profile: u32) -> anyhow::Result<()> { (**self).reset_fan_curves(profile).await }
    async fn set_fan_curves_enabled(&self, profile: u32, enabled: bool) -> anyhow::Result<()> { (**self).set_fan_curves_enabled(profile, enabled).await }
    async fn armoury_range(&self, attr: &str) -> anyhow::Result<(i32, i32)> { (**self).armoury_range(attr).await }
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()> { (**self).set_profile_epp(profile, epp).await }
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
