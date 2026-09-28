use crate::hw::{GHELPER_UNIT, Services};
use anyhow::{Context, bail};
use armoury_proto::ControlMode;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const PKEXEC: &str = "pkexec";
pub const ROOT_HELPER: &str = "/usr/local/lib/omarchy-armoury/armoury-root";
/// Also checked by the G-Helper autostart drop-in, so it is the single source of truth.
pub const ACTIVE_FLAG: &str = "active";

pub struct Control {
    flag: PathBuf,
    mode: ControlMode,
}

impl Control {
    pub fn load(state_dir: &Path) -> Self {
        let flag = state_dir.join(ACTIVE_FLAG);
        let mode = if flag.exists() { ControlMode::Active } else { ControlMode::Observe };
        Self { flag, mode }
    }

    pub fn mode(&self) -> ControlMode { self.mode }

    pub fn set(&mut self, mode: ControlMode) -> std::io::Result<()> {
        match mode {
            ControlMode::Active => {
                if let Some(dir) = self.flag.parent() { std::fs::create_dir_all(dir)?; }
                std::fs::write(&self.flag, b"")?;
            }
            ControlMode::Observe => match std::fs::remove_file(&self.flag) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            },
        }
        self.mode = mode;
        Ok(())
    }

    pub fn require_active(&self) -> anyhow::Result<()> {
        if self.mode != ControlMode::Active { bail!("observe mode: run 'armoury takeover' first"); }
        Ok(())
    }
}

pub async fn takeover(ctl: &mut Control, svc: &dyn Services) -> anyhow::Result<()> {
    if ctl.mode() == ControlMode::Active { return Ok(()); }
    let _ = svc.run(&["systemctl", "--user", "stop", GHELPER_UNIT]).await;
    if svc.is_running("ghelper").await {
        let _ = svc.run(&["pkill", "-x", "ghelper"]).await;
    }
    for _ in 0..20 {
        if !svc.is_running("ghelper").await { break; }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if svc.is_running("ghelper").await { bail!("G-Helper did not exit; takeover aborted"); }

    ctl.set(ControlMode::Active)?;
    if let Err(e) = svc.run(&[PKEXEC, ROOT_HELPER, "takeover"]).await {
        ctl.set(ControlMode::Observe)?;
        let _ = svc.run(&["systemctl", "--user", "start", GHELPER_UNIT]).await;
        return Err(e).context("takeover failed; G-Helper restored");
    }
    Ok(())
}

pub async fn handback(ctl: &mut Control, svc: &dyn Services) -> anyhow::Result<()> {
    if ctl.mode() == ControlMode::Observe { return Ok(()); }
    svc.run(&[PKEXEC, ROOT_HELPER, "handback"]).await.context("handback failed; still active")?;
    ctl.set(ControlMode::Observe)?;
    svc.run(&["systemctl", "--user", "start", GHELPER_UNIT]).await.context("asusd stopped but G-Helper did not start")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::FakeServices;

    fn ghelper_up() -> FakeServices {
        let s = FakeServices::default();
        s.running.lock().unwrap().insert("ghelper".into());
        s
    }

    #[test]
    fn load_and_set_persist_flag() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Control::load(d.path());
        assert_eq!(c.mode(), ControlMode::Observe);
        assert!(c.require_active().is_err());
        c.set(ControlMode::Active).unwrap();
        assert!(d.path().join(ACTIVE_FLAG).exists());
        assert_eq!(Control::load(d.path()).mode(), ControlMode::Active);
        c.set(ControlMode::Observe).unwrap();
        assert!(!d.path().join(ACTIVE_FLAG).exists());
    }

    #[tokio::test]
    async fn takeover_stops_ghelper_then_calls_root() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Control::load(d.path());
        let s = ghelper_up();
        takeover(&mut c, &s).await.unwrap();
        assert_eq!(c.mode(), ControlMode::Active);
        let calls = s.calls.lock().unwrap().clone();
        assert_eq!(calls, vec![
            format!("systemctl --user stop {GHELPER_UNIT}"),
            format!("{PKEXEC} {ROOT_HELPER} takeover"),
        ]);
    }

    #[tokio::test]
    async fn takeover_rolls_back_when_root_fails() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Control::load(d.path());
        let s = ghelper_up();
        *s.fail_on.lock().unwrap() = Some("pkexec".into());
        let err = takeover(&mut c, &s).await.unwrap_err();
        assert!(format!("{err:#}").contains("G-Helper restored"));
        assert_eq!(c.mode(), ControlMode::Observe);
        assert!(!d.path().join(ACTIVE_FLAG).exists());
        assert!(s.running.lock().unwrap().contains("ghelper"));
    }

    #[tokio::test]
    async fn handback_reverses_takeover() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Control::load(d.path());
        let s = ghelper_up();
        takeover(&mut c, &s).await.unwrap();
        handback(&mut c, &s).await.unwrap();
        assert_eq!(c.mode(), ControlMode::Observe);
        assert!(s.running.lock().unwrap().contains("ghelper"));
        let calls = s.calls.lock().unwrap().clone();
        assert_eq!(&calls[2..], &[
            format!("{PKEXEC} {ROOT_HELPER} handback"),
            format!("systemctl --user start {GHELPER_UNIT}"),
        ]);
    }

    #[tokio::test]
    async fn handback_stays_active_if_root_fails() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Control::load(d.path());
        c.set(ControlMode::Active).unwrap();
        let s = FakeServices::default();
        *s.fail_on.lock().unwrap() = Some("pkexec".into());
        assert!(handback(&mut c, &s).await.is_err());
        assert_eq!(c.mode(), ControlMode::Active);
    }

    #[tokio::test]
    async fn both_are_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Control::load(d.path());
        let s = FakeServices::default();
        handback(&mut c, &s).await.unwrap();
        assert!(s.calls.lock().unwrap().is_empty());
        c.set(ControlMode::Active).unwrap();
        takeover(&mut c, &s).await.unwrap();
        assert!(s.calls.lock().unwrap().is_empty());
    }
}
