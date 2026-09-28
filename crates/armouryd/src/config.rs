use armoury_proto::{ModeSettings, Profile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Re-write the current mode's power limits this often (0 = off); BIOS can clobber them.
    pub reapply_power_secs: u32,
    pub modes: BTreeMap<Profile, ModeSettings>,
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
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => (c, None),
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

    pub fn mode(&self, p: Profile) -> ModeSettings {
        self.modes.get(&p).copied().unwrap_or_default()
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
        c.modes.insert(Profile::Performance, ModeSettings { pl1: Some(120), pl2: Some(150), epp: Some(Epp::Performance), cpu_boost: Some(true), ..Default::default() });
        c.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("[modes.performance]"), "{text}");
        let (back, err) = Config::load(&p);
        assert!(err.is_none());
        assert_eq!(back, c);
        assert_eq!(back.mode(Profile::Quiet), ModeSettings::default());
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
        std::fs::write(&p, "modes = 7\n").unwrap();
        let (c, err) = Config::load(&p);
        assert_eq!(c, Config::default());
        assert!(err.unwrap().contains("config.toml"));
    }

    #[test]
    fn path_under_home() {
        assert_eq!(config_path(Path::new("/h")), Path::new("/h/.config/omarchy-armoury/config.toml"));
    }
}
