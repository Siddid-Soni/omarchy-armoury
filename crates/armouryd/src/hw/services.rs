use super::Services;
use anyhow::{Context, bail};
use tokio::process::Command;

pub struct RealServices {
    pub timeout: std::time::Duration,
}

impl Default for RealServices {
    fn default() -> Self { Self { timeout: std::time::Duration::from_secs(120) } }
}

#[async_trait::async_trait]
impl Services for RealServices {
    async fn run(&self, argv: &[&str]) -> anyhow::Result<()> {
        let (prog, args) = argv.split_first().context("empty argv")?;
        let child = Command::new(prog).args(args).kill_on_drop(true).output();
        let out = tokio::time::timeout(self.timeout, child).await
            .map_err(|_| anyhow::anyhow!("{} timed out after {:?}", argv.join(" "), self.timeout))?
            .with_context(|| format!("spawn {prog}"))?;
        if !out.status.success() {
            bail!("{} failed ({}): {}", argv.join(" "), out.status, String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    async fn is_running(&self, process: &str) -> bool {
        Command::new("pgrep").args(["-x", process]).output().await.map(|o| o.status.success()).unwrap_or(false)
    }

    async fn unit_active(&self, unit: &str, user: bool) -> bool {
        let mut c = Command::new("systemctl");
        if user { c.arg("--user"); }
        c.args(["is-active", "--quiet", unit]).status().await.map(|s| s.success()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn run_times_out() {
        let s = RealServices { timeout: Duration::from_millis(100) };
        let err = s.run(&["sleep", "5"]).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }
}
