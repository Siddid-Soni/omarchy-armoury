use super::Services;
use anyhow::{Context, bail};
use tokio::process::Command;

pub struct RealServices;

#[async_trait::async_trait]
impl Services for RealServices {
    async fn run(&self, argv: &[&str]) -> anyhow::Result<()> {
        let (prog, args) = argv.split_first().context("empty argv")?;
        let out = Command::new(prog).args(args).output().await.with_context(|| format!("spawn {prog}"))?;
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
