use super::{Asusd, GHELPER_UNIT, Gfx, Services, Sysfs};
use crate::features::fan::RawCurve;
use crate::features::lighting::{RawMode, RawPower};
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
    pub set_calls: Mutex<Vec<u32>>,
    /// set_mode returns an error (the call may still have been applied)
    pub set_fails: Mutex<bool>,
    /// PendingMode reported once set_mode has been called
    pub pending_after_fail: Mutex<Option<u32>>,
}

impl Default for FakeGfx {
    fn default() -> Self { Self { mode: 0, supported: vec![1, 0, 3, 5], pending: 6, power: 1, wedged: false, set_calls: Mutex::new(Vec::new()), set_fails: Mutex::new(false), pending_after_fail: Mutex::new(None) } }
}

impl FakeGfx {
    async fn gate(&self) { if self.wedged { std::future::pending::<()>().await } }
}

#[async_trait::async_trait]
impl Gfx for FakeGfx {
    async fn mode(&self) -> anyhow::Result<u32> { self.gate().await; Ok(self.mode) }
    async fn supported(&self) -> anyhow::Result<Vec<u32>> { self.gate().await; Ok(self.supported.clone()) }
    async fn pending_mode(&self) -> anyhow::Result<u32> {
        self.gate().await;
        let after = if self.set_calls.lock().unwrap().is_empty() { None } else { *self.pending_after_fail.lock().unwrap() };
        Ok(after.unwrap_or(self.pending))
    }
    async fn power(&self) -> anyhow::Result<u32> { self.gate().await; Ok(self.power) }
    async fn set_mode(&self, mode: u32) -> anyhow::Result<u32> {
        self.gate().await;
        self.set_calls.lock().unwrap().push(mode);
        if *self.set_fails.lock().unwrap() { anyhow::bail!("fake SetMode failure"); }
        Ok(1)
    }
}

#[derive(Default)]
pub struct FakeServices {
    pub calls: Mutex<Vec<String>>,
    pub running: Mutex<HashSet<String>>,
    pub active_units: Mutex<HashSet<String>>,
    pub fail_on: Mutex<Option<String>>,
    /// Canned stdout for `output`, keyed by a substring of the command.
    pub outputs: Mutex<HashMap<String, String>>,
}

#[async_trait::async_trait]
impl Services for FakeServices {
    async fn spawn(&self, argv: &[&str], _path_prepend: &str) -> anyhow::Result<()> {
        let joined = format!("spawn {}", argv.join(" "));
        self.calls.lock().unwrap().push(joined.clone());
        if let Some(f) = self.fail_on.lock().unwrap().as_deref() {
            if joined.contains(f) { anyhow::bail!("fake launch failure: {joined}"); }
        }
        Ok(())
    }

    async fn output(&self, argv: &[&str]) -> anyhow::Result<String> {
        self.run(argv).await?;
        let joined = argv.join(" ");
        Ok(self.outputs.lock().unwrap().iter().find(|(k, _)| joined.contains(k.as_str())).map(|(_, v)| v.clone()).unwrap_or_default())
    }
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

pub struct FakeAura {
    pub calls: Mutex<Vec<String>>,
    pub mode: Mutex<RawMode>,
    pub power: Mutex<RawPower>,
    pub brightness: Mutex<u32>,
    pub modes: Vec<u32>,
    pub zones: Vec<u32>,
    pub missing: bool,
    pub fail: Mutex<bool>,
}

impl Default for FakeAura {
    /// State captured from the G533ZW's asusd.
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            mode: Mutex::new((0, 0, (166, 0, 0), (0, 0, 0), "Med".into(), "Right".into())),
            power: Mutex::new((vec![(1, true, true, false, false), (2, true, true, false, false), (0, true, true, false, false)],)),
            brightness: Mutex::new(3),
            modes: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12],
            zones: vec![1, 2, 0],
            missing: false,
            fail: Mutex::new(false),
        }
    }
}

#[async_trait::async_trait]
impl super::Aura for FakeAura {
    async fn info(&self) -> anyhow::Result<(RawMode, RawPower, Vec<u32>, Vec<u32>)> {
        if self.missing { anyhow::bail!("asusd has no Aura keyboard device"); }
        Ok((self.mode.lock().unwrap().clone(), self.power.lock().unwrap().clone(), self.modes.clone(), self.zones.clone()))
    }
    async fn set_mode_data(&self, m: RawMode) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(format!("set_mode_data {}", m.0));
        *self.mode.lock().unwrap() = m;
        Ok(())
    }
    async fn set_power(&self, p: RawPower) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(format!("set_power {:?}", p.0));
        *self.power.lock().unwrap() = p;
        Ok(())
    }
    async fn set_brightness(&self, level: u32) -> anyhow::Result<()> {
        if *self.fail.lock().unwrap() { anyhow::bail!("fake asusd not ready"); }
        self.calls.lock().unwrap().push(format!("set_brightness {level}"));
        *self.brightness.lock().unwrap() = level;
        Ok(())
    }
}

pub struct FakeHypr {
    pub monitors: Mutex<Vec<armoury_proto::DisplayInfo>>,
    pub evals: Mutex<Vec<String>>,
    pub gamma: Mutex<Vec<u8>>,
    pub sunset_running: bool,
}

impl Default for FakeHypr {
    /// This machine's panel: eDP-1 2560x1440, 240/60 Hz, scale 1.6.
    fn default() -> Self {
        Self {
            monitors: Mutex::new(vec![armoury_proto::DisplayInfo {
                output: "eDP-1".into(), width: 2560, height: 1440, refresh_hz: 240.0, rates: vec![240.0, 60.0], scale: 1.6,
            }]),
            evals: Mutex::new(Vec::new()),
            gamma: Mutex::new(Vec::new()),
            sunset_running: true,
        }
    }
}

#[async_trait::async_trait]
impl super::hypr::Hypr for FakeHypr {
    async fn monitors(&self) -> anyhow::Result<Vec<armoury_proto::DisplayInfo>> { Ok(self.monitors.lock().unwrap().clone()) }
    async fn touchpad(&self) -> Option<String> { Some("asue1403:00-04f3:319a-touchpad".into()) }
    async fn eval(&self, lua: &str) -> anyhow::Result<()> {
        self.evals.lock().unwrap().push(lua.to_string());
        // mirror a refresh change so later reads see it
        if let Some(hz) = lua.split('@').nth(1).and_then(|s| s.split('"').next()).and_then(|s| s.parse::<f32>().ok()) {
            for m in self.monitors.lock().unwrap().iter_mut() { m.refresh_hz = hz; }
        }
        Ok(())
    }
    async fn gamma(&self, pct: u8) -> anyhow::Result<()> {
        if !self.sunset_running { anyhow::bail!("hyprsunset is not running (turn on Omarchy's night light, or start hyprsunset)"); }
        self.gamma.lock().unwrap().push(pct);
        Ok(())
    }
}

#[derive(Default)]
pub struct FakeNvidia {
    pub active: Mutex<Option<bool>>,
    pub status_reads: std::sync::atomic::AtomicU32,
}

impl FakeNvidia {
    pub fn with_active(active: Option<bool>) -> Self {
        Self { active: Mutex::new(active), ..Default::default() }
    }
}

impl super::Nvidia for FakeNvidia {
    fn dgpu_active(&self) -> Option<bool> { *self.active.lock().unwrap() }
    fn status(&self) -> Option<armoury_proto::NvStatus> {
        self.status_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(armoury_proto::NvStatus { core_mhz: 1500, pstate: "P0".into(), ..Default::default() })
    }
    fn users(&self) -> Vec<armoury_proto::GpuUser> { vec![armoury_proto::GpuUser { pid: 42, name: "game".into() }] }
}

impl<T: super::Nvidia + ?Sized> super::Nvidia for std::sync::Arc<T> {
    fn dgpu_active(&self) -> Option<bool> { (**self).dgpu_active() }
    fn status(&self) -> Option<armoury_proto::NvStatus> { (**self).status() }
    fn users(&self) -> Vec<armoury_proto::GpuUser> { (**self).users() }
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
    async fn set_fan_curves_enabled(&self, profile: u32, enabled: bool) -> anyhow::Result<()> {
        self.record(format!("set_fan_curves_enabled {profile} {enabled}"))
    }
    async fn armoury_range(&self, _attr: &str) -> anyhow::Result<(i32, i32)> { Ok((-1, -1)) }
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()> { self.record(format!("set_profile_epp {profile} {epp}")) }
    async fn set_charge_limit(&self, percent: u8) -> anyhow::Result<()> { self.record(format!("set_charge_limit {percent}")) }
    async fn one_shot_charge(&self) -> anyhow::Result<()> { self.record("one_shot_charge".into()) }
    async fn set_source_profiles(&self, ac: Option<u32>, battery: Option<u32>) -> anyhow::Result<()> {
        self.record(format!("set_source_profiles {ac:?} {battery:?}"))
    }
    async fn armoury_set_value(&self, attr: &str, value: i32) -> anyhow::Result<()> { self.record(format!("armoury_set_value {attr} {value}")) }
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

    #[tokio::test]
    async fn fake_gfx_set_mode() {
        use crate::hw::Gfx;
        let g = FakeGfx::default();
        assert_eq!(g.set_mode(5).await.unwrap(), 1);
        assert_eq!(*g.set_calls.lock().unwrap(), [5]);
    }

    #[tokio::test]
    async fn fake_aura_holds_state() {
        use crate::hw::Aura;
        let a = FakeAura::default();
        let (mode, power, modes, zones) = a.info().await.unwrap();
        assert_eq!(mode.0, 0);
        assert_eq!(power.0.len(), 3);
        assert_eq!(modes.len(), 12);
        assert_eq!(zones, [1, 2, 0]);
        a.set_brightness(1).await.unwrap();
        assert_eq!(*a.brightness.lock().unwrap(), 1);
        assert_eq!(a.calls.lock().unwrap()[0], "set_brightness 1");
    }

    #[tokio::test]
    async fn fake_asusd_system_calls() {
        use crate::hw::Asusd;
        let a = FakeAsusd::default();
        a.set_charge_limit(80).await.unwrap();
        a.one_shot_charge().await.unwrap();
        a.set_source_profiles(Some(1), None).await.unwrap();
        a.armoury_set_value("panel_overdrive", 0).await.unwrap();
        assert_eq!(*a.calls.lock().unwrap(), ["set_charge_limit 80", "one_shot_charge", "set_source_profiles Some(1) None", "armoury_set_value panel_overdrive 0"]);
    }
}
