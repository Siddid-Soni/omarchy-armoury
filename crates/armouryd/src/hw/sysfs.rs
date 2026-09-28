use super::Sysfs;
use std::path::PathBuf;

pub const PRODUCT_NAME: &str = "sys/class/dmi/id/product_name";
pub const KEYSTONE: &str = "sys/devices/platform/asus-nb-wmi/keystone";
pub const GPU_MUX: &str = "sys/devices/platform/asus-nb-wmi/gpu_mux_mode";
pub const DGPU_DISABLE: &str = "sys/devices/platform/asus-nb-wmi/dgpu_disable";
pub const PLATFORM_PROFILE: &str = "sys/firmware/acpi/platform_profile";
pub const POWER_SUPPLY_DIR: &str = "sys/class/power_supply";

pub struct RealSysfs {
    root: PathBuf,
}

impl RealSysfs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Sysfs for RealSysfs {
    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok().map(|s| s.trim().to_string())
    }

    fn list(&self, rel: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root.join(rel))
            .map(|rd| rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::Sysfs;

    #[test]
    fn reads_trimmed_and_lists_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sys/class/power_supply");
        std::fs::create_dir_all(p.join("BAT0")).unwrap();
        std::fs::create_dir_all(p.join("ADP0")).unwrap();
        std::fs::write(p.join("BAT0/capacity"), "80\n").unwrap();
        let s = RealSysfs::new(dir.path());
        assert_eq!(s.read("sys/class/power_supply/BAT0/capacity").as_deref(), Some("80"));
        assert_eq!(s.read("missing"), None);
        assert_eq!(s.list(POWER_SUPPLY_DIR), vec!["ADP0", "BAT0"]);
        assert!(s.list("missing").is_empty());
    }
}
