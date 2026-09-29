use armoury_proto::{ModeSettings, Profile};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Re-write the current mode's power limits this often (0 = off); BIOS can clobber them.
    pub reapply_power_secs: u32,
    /// Pre-Manual per-mode settings: accepted on load and dropped (tuning lives in manual profiles).
    #[serde(skip_serializing)]
    pub modes: Option<toml::Value>,
    pub manual: ManualConfig,
    pub lighting: LightingConfig,
    pub system: SystemConfig,
    pub keys: KeysConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeysConfig {
    pub rog: armoury_proto::KeyAction,
    pub fan: armoury_proto::KeyAction,
    pub aura: armoury_proto::KeyAction,
    pub rog_command: Option<String>,
    pub fan_command: Option<String>,
    pub aura_command: Option<String>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            rog: armoury_proto::KeyAction::OpenWindow,
            fan: armoury_proto::KeyAction::CycleMode,
            aura: armoury_proto::KeyAction::CycleEffect,
            rog_command: None,
            fan_command: None,
            aura_command: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SystemConfig {
    /// Re-applied at every active start (the kernel resets it each boot).
    pub sleep_mode: Option<armoury_proto::SleepMode>,
    /// Stay awake with the lid closed while on AC.
    pub clamshell: bool,
    /// Built-in panel refresh rate on AC / on battery.
    pub refresh_ac: Option<f32>,
    pub refresh_battery: Option<f32>,
    /// Mode asusd switches to on AC / on battery (kept here so the UI can show it while asusd is stopped).
    pub profile_ac: Option<armoury_proto::ModeChoice>,
    pub profile_battery: Option<armoury_proto::ModeChoice>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LightingConfig {
    /// Keyboard brightness (0–3) to use on AC / on battery; None = leave as is.
    pub brightness_ac: Option<u8>,
    pub brightness_battery: Option<u8>,
    /// Never dim the keyboard when idle.
    pub keep_on: bool,
}

pub fn config_path(home: &Path) -> PathBuf {
    home.join(".config/omarchy-armoury/config.toml")
}

impl Config {
    /// A missing file is the default config; an unreadable one is the default plus the error.
    pub fn load(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
            Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut c) => {
                    if c.modes.take().is_some() { eprintln!("armouryd: ignoring the old [modes] table; tuning now lives in manual profiles"); }
                    (c, None)
                }
                Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
            },
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// Stock settings for every mode until the manual-mode apply loop lands (Task 3 removes this).
    pub fn mode(&self, _p: Profile) -> ModeSettings { ModeSettings::default() }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ManualConfig {
    /// Manual is the current mode (re-entered at start and takeover).
    pub enabled: bool,
    pub active: Option<String>,
    pub profiles: Vec<armoury_proto::ManualProfile>,
}

impl ManualConfig {
    pub fn active_profile(&self) -> Option<&armoury_proto::ManualProfile> {
        self.active.as_deref().and_then(|a| self.profiles.iter().find(|p| p.name == a)).or(self.profiles.first())
    }

    /// Creates or overwrites `p`; with `original` set to another name, renames that profile in place.
    pub fn save(&mut self, mut p: armoury_proto::ManualProfile, original: Option<&str>) -> Result<(), String> {
        p.name = p.name.trim().to_string();
        if p.name.is_empty() || p.name.chars().count() > 40 { return Err("profile name must be 1–40 characters".into()); }
        match original.filter(|o| *o != p.name) {
            Some(old) => {
                if self.profiles.iter().any(|q| q.name == p.name) { return Err(format!("a profile named {} already exists", p.name)); }
                let i = self.profiles.iter().position(|q| q.name == old).ok_or_else(|| format!("no profile named {old}"))?;
                if self.active.as_deref() == Some(old) { self.active = Some(p.name.clone()); }
                self.profiles[i] = p;
            }
            None => match self.profiles.iter().position(|q| q.name == p.name) {
                Some(i) => self.profiles[i] = p,
                None => self.profiles.push(p),
            },
        }
        if self.active.is_none() { self.active = self.profiles.first().map(|q| q.name.clone()); }
        Ok(())
    }

    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let i = self.profiles.iter().position(|q| q.name == name).ok_or_else(|| format!("no profile named {name}"))?;
        if self.profiles.len() == 1 { return Err("can't delete the last manual profile".into()); }
        self.profiles.remove(i);
        if self.active.as_deref() == Some(name) { self.active = self.profiles.first().map(|q| q.name.clone()); }
        Ok(())
    }

    /// Makes `name` active and turns Manual on.
    pub fn activate(&mut self, name: &str) -> Result<(), String> {
        if !self.profiles.iter().any(|q| q.name == name) { return Err(format!("no profile named {name}")); }
        self.active = Some(name.to_string());
        self.enabled = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use armoury_proto::Epp;

    #[test]
    fn round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/config.toml");
        let mut c = Config::default();
        c.reapply_power_secs = 30;
        let mut prof = prof("Gaming");
        prof.settings = ModeSettings { pl1: Some(120), pl2: Some(150), epp: Some(Epp::Performance), cpu_boost: Some(true), ..Default::default() };
        c.manual.save(prof, None).unwrap();
        c.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("[[manual.profiles]]"), "{text}");
        let (back, err) = Config::load(&p);
        assert!(err.is_none());
        assert_eq!(back, c);
    }

    #[test]
    fn missing_is_default() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(&d.path().join("nope.toml")), (Config::default(), None));
    }

    #[test]
    fn corrupt_config_falls_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        std::fs::write(&p, "reapply_power_secs = \"often\"\n").unwrap();
        let (c, err) = Config::load(&p);
        assert_eq!(c, Config::default());
        assert!(err.unwrap().contains("config.toml"));
    }

    #[test]
    fn path_under_home() {
        assert_eq!(config_path(Path::new("/h")), Path::new("/h/.config/omarchy-armoury/config.toml"));
    }

    #[test]
    fn unknown_keys_are_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        std::fs::write(&p, "[lighting]\nkeep_onn = true\n").unwrap();
        assert!(Config::load(&p).1.unwrap().contains("keep_onn"));
    }

    fn prof(name: &str) -> armoury_proto::ManualProfile {
        serde_json::from_value(serde_json::json!({"name": name})).unwrap()
    }

    #[test]
    fn manual_rules() {
        let mut m = ManualConfig::default();
        m.save(prof("A"), None).unwrap();
        assert_eq!(m.active.as_deref(), Some("A"), "first profile becomes active");
        m.save(prof("B"), None).unwrap();
        assert!(m.save(prof(" "), None).is_err(), "blank name");
        assert!(m.save(prof("B"), Some("A")).is_err(), "rename onto an existing name");
        m.save(prof("C"), Some("A")).unwrap(); // rename the active one
        assert_eq!(m.active.as_deref(), Some("C"));
        assert_eq!(m.profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["C", "B"], "rename keeps position");
        m.activate("B").unwrap();
        assert!(m.enabled && m.active.as_deref() == Some("B"));
        assert!(m.activate("nope").is_err());
        m.delete("B").unwrap();
        assert_eq!(m.active.as_deref(), Some("C"), "deleting the active one falls back to the first");
        assert!(m.delete("C").unwrap_err().contains("last"));
        assert!(m.delete("nope").is_err());
    }

    #[test]
    fn manual_round_trip_and_legacy_modes_ignored() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let mut c = Config::default();
        let mut p = prof("Gaming");
        p.settings.pl1 = Some(90);
        p.curves.push(armoury_proto::FanCurve { fan: armoury_proto::Fan::Cpu, temps: [30, 40, 50, 60, 70, 80, 90, 100], percent: [0, 10, 20, 35, 55, 75, 90, 100], enabled: true });
        c.manual.save(p, None).unwrap();
        c.manual.enabled = true;
        c.system.profile_battery = Some(armoury_proto::ModeChoice::Manual);
        c.save(&path).unwrap();
        let (back, err) = Config::load(&path);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(back.manual, c.manual);
        assert_eq!(back.system.profile_battery, Some(armoury_proto::ModeChoice::Manual));
        std::fs::write(&path, "[modes.balanced]\npl1 = 60\n").unwrap();
        let (old, err) = Config::load(&path);
        assert!(err.is_none(), "old [modes] must not make the config bad: {err:?}");
        assert!(old.modes.is_none(), "legacy table dropped after load");
    }
}
