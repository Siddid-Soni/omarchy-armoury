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
    pub numpad: NumpadConfig,
    pub music: MusicConfig,
    pub keystone: KeystoneConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeystoneConfig {
    pub insert: armoury_proto::KeystoneAction,
    pub remove: armoury_proto::KeystoneAction,
    /// Flash the Keystone LED when it goes in.
    pub flash: bool,
}

impl Default for KeystoneConfig {
    fn default() -> Self { Self { insert: Default::default(), remove: Default::default(), flash: true } }
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NumpadConfig {
    /// Backlight level 1–8 used every time the NumberPad turns on.
    pub start_brightness: u8,
    /// Let the NumberPad work while Omarchy's touchpad toggle is off.
    pub allow_when_touchpad_off: bool,
    /// Backlight off after this long without a touch (0 = never).
    pub idle_dim_secs: u32,
    /// Top-right icon hold that toggles the NumberPad.
    pub hold_ms: u32,
    /// A finger resting on a key repeats it.
    pub key_repeat: bool,
    /// Delay before the first repeat (0 = the keyboard's, from Hyprland).
    pub repeat_delay_ms: u32,
    /// Repeats per second after the delay (0 = the keyboard's own rate, from Hyprland).
    pub repeat_rate_hz: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MusicConfig {
    /// Music lighting was left on: it starts again in active mode.
    pub on: bool,
    pub style: armoury_proto::MusicStyle,
    pub scheme: armoury_proto::MusicScheme,
    pub colour1: [u8; 3],
    pub colour2: [u8; 3],
    /// 1–10: how far below the peak still lights up.
    pub sensitivity: u8,
}

impl Default for MusicConfig {
    fn default() -> Self {
        Self { on: false, style: Default::default(), scheme: Default::default(), colour1: [0x00, 0xc8, 0xff], colour2: [0xff, 0x00, 0x40], sensitivity: 5 }
    }
}

impl MusicConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=10).contains(&self.sensitivity) { return Err("music sensitivity must be 1–10".into()); }
        Ok(())
    }
}

impl Default for NumpadConfig {
    fn default() -> Self { Self { start_brightness: 8, allow_when_touchpad_off: false, idle_dim_secs: 60, hold_ms: 1000, key_repeat: true, repeat_delay_ms: 0, repeat_rate_hz: 0 } }
}

impl NumpadConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=8).contains(&self.start_brightness) { return Err("NumberPad brightness must be 1–8".into()); }
        if !(300..=3000).contains(&self.hold_ms) { return Err("hold time must be 300–3000 ms".into()); }
        if self.idle_dim_secs > 3600 { return Err("idle timeout must be 0–3600 s".into()); }
        if self.repeat_delay_ms != 0 && !(100..=2000).contains(&self.repeat_delay_ms) { return Err("key repeat delay must be 0 (keyboard's) or 100–2000 ms".into()); }
        if self.repeat_rate_hz > 100 { return Err("key repeat rate must be 0 (keyboard's) to 100 per second".into()); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use armoury_proto::{Epp, ModeSettings};

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

    #[test]
    fn keystone_config_round_trip() {
        let (cfg, err) = { let d = tempfile::tempdir().unwrap(); let p = d.path().join("c.toml");
            std::fs::write(&p, "[keystone.insert]\nmode = \"performance\"\nlight = \"music\"\n[keystone.remove]\nlight = \"previous\"\nlock = true\n").unwrap(); Config::load(&p) };
        assert!(err.is_none(), "{err:?}");
        assert_eq!(cfg.keystone.insert.mode, Some(armoury_proto::ModeChoice::Performance));
        assert_eq!(cfg.keystone.insert.light, armoury_proto::KeystoneLight::Music);
        assert!(cfg.keystone.remove.lock && cfg.keystone.flash, "flash defaults on");
        let text = toml::to_string(&cfg).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap().keystone, cfg.keystone);
    }

    #[test]
    fn music_defaults_and_validation() {
        let c = MusicConfig::default();
        assert!(!c.on && c.sensitivity == 5 && c.validate().is_ok());
        assert!(MusicConfig { sensitivity: 0, ..c }.validate().is_err());
        assert!(MusicConfig { sensitivity: 11, ..c }.validate().is_err());
        let (cfg, err) = { let d = tempfile::tempdir().unwrap(); let p = d.path().join("c.toml");
            std::fs::write(&p, "[music]\non = true\nstyle = \"pulse\"\n").unwrap(); Config::load(&p) };
        assert!(err.is_none());
        assert_eq!((cfg.music.on, cfg.music.style, cfg.music.sensitivity), (true, armoury_proto::MusicStyle::Pulse, 5));
    }

    #[test]
    fn numpad_defaults_and_validation() {
        let c = NumpadConfig::default();
        assert_eq!((c.start_brightness, c.allow_when_touchpad_off, c.idle_dim_secs, c.hold_ms), (8, false, 60, 1000));
        assert_eq!((c.key_repeat, c.repeat_delay_ms, c.repeat_rate_hz), (true, 0, 0), "repeat on, delay and rate = the keyboard's");
        assert!(NumpadConfig { repeat_delay_ms: 0, ..c }.validate().is_ok());
        assert!(NumpadConfig { repeat_delay_ms: 50, ..c }.validate().is_err(), "below 100 ms");
        assert_eq!(c.repeat_rate_hz, 0, "0 = the keyboard's own repeat rate");
        assert!(NumpadConfig { repeat_rate_hz: 30, ..c }.validate().is_ok());
        assert!(NumpadConfig { repeat_rate_hz: 101, ..c }.validate().is_err());
        assert!(c.validate().is_ok());
        for bad in [NumpadConfig { start_brightness: 0, ..c }, NumpadConfig { start_brightness: 9, ..c },
                    NumpadConfig { hold_ms: 200, ..c }, NumpadConfig { hold_ms: 3001, ..c }, NumpadConfig { idle_dim_secs: 3601, ..c }] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let (cfg, err) = { let d = tempfile::tempdir().unwrap(); let p = d.path().join("c.toml");
            std::fs::write(&p, "[numpad]\nidle_dim_secs = 0\n").unwrap(); Config::load(&p) };
        assert!(err.is_none());
        assert_eq!((cfg.numpad.idle_dim_secs, cfg.numpad.start_brightness), (0, 8), "missing keys take defaults");
    }
}
