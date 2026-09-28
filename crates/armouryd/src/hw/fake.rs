use super::{Asusd, GHELPER_UNIT, Gfx, Services, Sysfs};
use crate::features::fan::RawCurve;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Default)]
pub struct FakeSysfs {
    pub files: Mutex<HashMap<String, String>>,
}

impl FakeSysfs {
    pub fn with(entries: &[(&str, &str)]) -> Self {
        let f = Self::default();
        for (k, v) in entries { f.files.lock().unwrap().insert((*k).into(), (*v).into()); }
        f
    }
}

impl Sysfs for FakeSysfs {
    fn read(&self, rel: &str) -> Option<String> { self.files.lock().unwrap().get(rel).cloned() }
    fn list(&self, rel: &str) -> Vec<String> {
        let prefix = format!("{rel}/");
        let mut v: Vec<String> = self.files.lock().unwrap().keys()
            .filter_map(|k| k.strip_prefix(&prefix)?.split('/').next().map(String::from))
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

pub struct FakeGfx {
    pub mode: u32,
    pub supported: Vec<u32>,
    pub pending: u32,
    pub power: u32,
    pub wedged: bool,
}

impl Default for FakeGfx {
    fn default() -> Self { Self { mode: 0, supported: vec![1, 0, 3, 5], pending: 6, power: 1, wedged: false } }
}

impl FakeGfx {
    async fn gate(&self) { if self.wedged { std::future::pending::<()>().await } }
}

#[async_trait::async_trait]
impl Gfx for FakeGfx {
    async fn mode(&self) -> anyhow::Result<u32> { self.gate().await; Ok(self.mode) }
    async fn supported(&self) -> anyhow::Result<Vec<u32>> { self.gate().await; Ok(self.supported.clone()) }
    async fn pending_mode(&self) -> anyhow::Result<u32> { self.gate().await; Ok(self.pending) }
    async fn power(&self) -> anyhow::Result<u32> { self.gate().await; Ok(self.power) }
}

#[derive(Default)]
pub struct FakeServices {
    pub calls: Mutex<Vec<String>>,
    pub running: Mutex<HashSet<String>>,
    pub active_units: Mutex<HashSet<String>>,
    pub fail_on: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl Services for FakeServices {
    async fn run(&self, argv: &[&str]) -> anyhow::Result<()> {
        let joined = argv.join(" ");
        self.calls.lock().unwrap().push(joined.clone());
        if let Some(f) = self.fail_on.lock().unwrap().as_deref() {
            if joined.contains(f) { anyhow::bail!("fake failure: {joined}"); }
        }
        let stop = format!("systemctl --user stop {GHELPER_UNIT}");
        let start = format!("systemctl --user start {GHELPER_UNIT}");
        if joined == stop || joined == "pkill -x ghelper" { self.running.lock().unwrap().remove("ghelper"); }
        if joined == start { self.running.lock().unwrap().insert("ghelper".into()); }
        Ok(())
    }
    async fn is_running(&self, process: &str) -> bool { self.running.lock().unwrap().contains(process) }
    async fn unit_active(&self, unit: &str, _user: bool) -> bool { self.active_units.lock().unwrap().contains(unit) }
}

#[derive(Default)]
pub struct FakeAsusd {
    pub calls: Mutex<Vec<String>>,
    pub curves: Mutex<HashMap<u32, Vec<RawCurve>>>,
    pub fail_on: Mutex<Option<String>>,
}

impl FakeAsusd {
    fn record(&self, s: String) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(s.clone());
        if let Some(f) = self.fail_on.lock().unwrap().as_deref() {
            if s.contains(f) { anyhow::bail!("fake asusd failure: {s}"); }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Asusd for FakeAsusd {
    async fn set_profile(&self, p: u32) -> anyhow::Result<()> { self.record(format!("set_profile {p}")) }
    async fn next_profile(&self) -> anyhow::Result<()> { self.record("next_profile".into()) }
    async fn fan_curves(&self, profile: u32) -> anyhow::Result<Vec<RawCurve>> {
        Ok(self.curves.lock().unwrap().get(&profile).cloned().unwrap_or_default())
    }
    async fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> anyhow::Result<()> {
        self.record(format!("set_fan_curve {profile} {}", curve.0))?;
        let mut all = self.curves.lock().unwrap();
        let list = all.entry(profile).or_default();
        list.retain(|c| c.0 != curve.0);
        list.push(curve);
        Ok(())
    }
    async fn reset_fan_curves(&self, profile: u32) -> anyhow::Result<()> { self.record(format!("reset_fan_curves {profile}")) }
    async fn armoury_range(&self, _attr: &str) -> anyhow::Result<(i32, i32)> { Ok((-1, -1)) }
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()> { self.record(format!("set_profile_epp {profile} {epp}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::{Services, Sysfs};

    #[tokio::test]
    async fn fake_services_model_ghelper_lifecycle() {
        let s = FakeServices::default();
        s.running.lock().unwrap().insert("ghelper".into());
        s.run(&["systemctl", "--user", "stop", GHELPER_UNIT]).await.unwrap();
        assert!(!s.is_running("ghelper").await);
        s.run(&["systemctl", "--user", "start", GHELPER_UNIT]).await.unwrap();
        assert!(s.is_running("ghelper").await);
        *s.fail_on.lock().unwrap() = Some("pkexec".into());
        assert!(s.run(&["pkexec", "x"]).await.is_err());
        assert_eq!(s.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn fake_sysfs_reads() {
        let f = FakeSysfs::with(&[("a/b", "1")]);
        assert_eq!(f.read("a/b").as_deref(), Some("1"));
        assert_eq!(f.list("a"), vec!["b"]);
    }

    #[tokio::test]
    async fn fake_asusd_records_and_fails() {
        use crate::hw::Asusd;
        let a = FakeAsusd::default();
        a.set_profile_epp(1, 1).await.unwrap();
        *a.fail_on.lock().unwrap() = Some("epp 0".into());
        assert!(a.set_profile_epp(0, 3).await.is_err());
        a.curves.lock().unwrap().insert(1, vec![("CPU".into(), [0; 8], [0; 8], true)]);
        assert_eq!(a.fan_curves(1).await.unwrap().len(), 1);
        assert_eq!(a.armoury_range("x").await.unwrap(), (-1, -1));
        assert_eq!(a.calls.lock().unwrap()[0], "set_profile_epp 1 1");
    }
}
