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
        self.output(argv).await.map(|_| ())
    }

    async fn output(&self, argv: &[&str]) -> anyhow::Result<String> {
        let (prog, args) = argv.split_first().context("empty argv")?;
        // the session's environment too: omarchy-osd and omarchy-shell need OMARCHY_PATH and
        // exit 0 without it, so a missing OSD or window would leave no trace
        let child = Command::new(prog).args(args).envs(session().await).kill_on_drop(true).output();
        let out = tokio::time::timeout(self.timeout, child).await
            .map_err(|_| anyhow::anyhow!("{} timed out after {:?}", argv.join(" "), self.timeout))?
            .with_context(|| format!("spawn {prog}"))?;
        if !out.status.success() {
            bail!("{} failed ({}): {}", argv.join(" "), out.status, String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn spawn(&self, argv: &[&str], path_prepend: &str) -> anyhow::Result<()> {
        let (prog, args) = argv.split_first().context("empty argv")?;
        if !std::path::Path::new(prog).exists() { bail!("cannot launch {prog}: not found"); }
        let path = format!("{path_prepend}:{}", std::env::var("PATH").unwrap_or_default());
        let st = Command::new("setsid").arg("-f").arg(prog).args(args).envs(session().await).env("PATH", path)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().await.with_context(|| format!("launch {prog}"))?;
        if !st.success() { bail!("launch {prog} failed ({st})"); }
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

/// The graphical session's environment, read on every call. armouryd can start before the
/// session exists, so its own environment lacks WAYLAND_DISPLAY, OMARCHY_PATH and friends (a
/// terminal launched from it dies, omarchy-osd shows nothing). The systemd user manager has
/// them (uwsm imports them), and gets the new ones when Hyprland restarts.
async fn session() -> Vec<(String, String)> {
    let out = Command::new("systemctl").args(["--user", "show-environment"]).kill_on_drop(true).output();
    match tokio::time::timeout(std::time::Duration::from_secs(5), out).await {
        Ok(Ok(o)) if o.status.success() => session_env(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// KEY=value lines of `systemctl --user show-environment`; values systemd shell-quotes ($'…') are skipped.
fn session_env(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, v)| !k.is_empty() && !v.starts_with("$'") && *k != "PATH")
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
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

    #[test]
    fn session_env_parses_plain_values_only() {
        let e = session_env("WAYLAND_DISPLAY=wayland-1\nHYPRLAND_CMD=$'Hyprland --watchdog-fd 4'\nPATH=/x\nA=b=c\n");
        assert_eq!(e, vec![("WAYLAND_DISPLAY".into(), "wayland-1".into()), ("A".into(), "b=c".into())]);
    }
}
