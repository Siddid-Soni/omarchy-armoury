pub const SUPPORT_FILE: &str = "/usr/share/asusd/aura_support.ron";
pub const AURA_CFG_DIR: &str = "/etc/asusd";
pub const STATE_DIR: &str = "/var/lib/omarchy-armoury";
pub const BOARD_NAME: &str = "/sys/class/dmi/id/board_name";
pub const NO_TURBO: &str = "/sys/devices/system/cpu/intel_pstate/no_turbo";

/// `set-limits KEY=VALUE…` → (sysfs node, value) pairs, in argument order.
/// Hard caps = g-helper-linux's widest non-special-edition ranges (HX Intel: 175 W).
pub fn parse_limits(args: &[String]) -> Result<Vec<(&'static str, String)>, String> {
    if args.is_empty() { return Err("set-limits: nothing to set".into()); }
    args.iter().map(|a| {
        let (k, v) = a.split_once('=').ok_or_else(|| format!("expected KEY=VALUE, got {a:?}"))?;
        let int = |lo: i32, hi: i32| -> Result<String, String> {
            v.parse::<i32>().ok().filter(|n| (lo..=hi).contains(n)).map(|n| n.to_string())
                .ok_or_else(|| format!("{k} must be {lo}–{hi}, got {v:?}"))
        };
        Ok(match k {
            "pl1" => ("/sys/devices/platform/asus-nb-wmi/ppt_pl1_spl", int(5, 175)?),
            "pl2" => ("/sys/devices/platform/asus-nb-wmi/ppt_pl2_sppt", int(5, 175)?),
            "nv_boost" => ("/sys/devices/platform/asus-nb-wmi/nv_dynamic_boost", int(5, 25)?),
            "nv_temp" => ("/sys/devices/platform/asus-nb-wmi/nv_temp_target", int(75, 87)?),
            "cpu_boost" => (NO_TURBO, no_turbo_value(v)?.to_string()),
            other => return Err(format!("unknown limit {other:?}")),
        })
    }).collect()
}

/// Attempts every write even after a failure; returns "node: error" for each failure.
pub fn write_all(pairs: &[(&'static str, String)], mut write: impl FnMut(&str, &str) -> Result<(), String>) -> Vec<String> {
    pairs.iter().filter_map(|(node, value)| write(node, value).err().map(|e| format!("{node}: {e}"))).collect()
}

/// armouryd's active-mode flag for the calling user; set-limits refuses without it
/// so a same-user process cannot change limits while G-Helper owns the hardware.
pub fn active_flag(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".local/state/omarchy-armoury/active")
}

/// `on|off` → value for intel_pstate/no_turbo.
pub fn no_turbo_value(state: &str) -> Result<&'static str, String> {
    match state {
        "on" => Ok("0"),
        "off" => Ok("1"),
        other => Err(format!("cpu-boost takes on|off, got {other:?}")),
    }
}

pub struct Fix {
    pub device: &'static str,
    pub zones: &'static [&'static str],
}

/// asusd's database under-reports power zones for these models (verified against G-Helper's firmware probe).
pub const FIXES: &[Fix] = &[Fix { device: "G533Z", zones: &["Keyboard", "Lightbar", "Logo"] }];

pub fn fix_for_board(board: &str) -> Option<&'static Fix> {
    FIXES.iter().find(|f| board.starts_with(f.device))
}

/// Adds the fix's zones to its model's `power_zones`, keeping any already listed.
/// `Ok(None)` means nothing to change.
pub fn patch_support(text: &str, fix: &Fix) -> Result<Option<String>, String> {
    let needle = format!("device_name: \"{}\",", fix.device);
    let start = text.find(&needle).ok_or_else(|| format!("no entry for {} in support file", fix.device))?;
    let after = start + needle.len();
    let entry_end = text[after..].find("device_name:").map_or(text.len(), |i| after + i);
    let entry = &text[start..entry_end];

    const KEY: &str = "power_zones: [";
    let open = entry.find(KEY).ok_or_else(|| format!("{} entry has no power_zones", fix.device))? + KEY.len();
    let close = entry[open..].find(']').ok_or_else(|| format!("{} power_zones is unterminated", fix.device))? + open;

    let mut zones: Vec<&str> = entry[open..close].split(',').map(str::trim).filter(|z| !z.is_empty()).collect();
    let before = zones.len();
    for z in fix.zones {
        if !zones.contains(z) { zones.push(z); }
    }
    if zones.len() == before { return Ok(None); }
    Ok(Some(format!("{}{}{}", &text[..start + open], zones.join(", "), &text[start + close..])))
}

/// True if an asusd aura config lacks a power state for any of the fix's zones.
pub fn aura_config_stale(text: &str, fix: &Fix) -> bool {
    fix.zones.iter().any(|z| !text.contains(&format!("zone: {z},")))
}

/// Configs to set aside so asusd regenerates them with all zones. Only after the
/// support file actually changed, so repeat runs never touch the user's config.
pub fn stale_configs<'a>(support_changed: bool, configs: &'a [(String, String)], fix: &Fix) -> Vec<&'a str> {
    if !support_changed { return Vec::new(); }
    configs.iter().filter(|(_, text)| aura_config_stale(text, fix)).map(|(name, _)| name.as_str()).collect()
}

/// First free `<path>.armoury-bak[.N]`, so an existing backup is never overwritten.
pub fn backup_path(path: &std::path::Path, taken: impl Fn(&std::path::Path) -> bool) -> std::path::PathBuf {
    let base = format!("{}.armoury-bak", path.display());
    let mut candidate = std::path::PathBuf::from(&base);
    let mut n = 0;
    while taken(&candidate) {
        n += 1;
        candidate = format!("{base}.{n}").into();
    }
    candidate
}

/// Pacman cache path of the installed asusctl, from `pacman -Q asusctl` output.
pub fn cached_package(pacman_q: &str, arch: &str) -> Option<String> {
    let version = pacman_q.split_whitespace().nth(1)?;
    Some(format!("/var/cache/pacman/pkg/asusctl-{version}-{arch}.pkg.tar.zst"))
}

/// System operations armoury-root performs, so takeover/handback ordering is testable.
pub trait Host {
    fn systemctl(&mut self, args: &[&str]) -> anyhow::Result<()>;
    fn asusd_masked(&mut self) -> bool;
    fn set_marker(&mut self, present: bool) -> anyhow::Result<()>;
    fn marker_exists(&mut self) -> bool;
    fn support_fix(&mut self) -> anyhow::Result<()>;
    fn warn(&mut self, msg: &str);
}

/// Unmask (remembering the mask) and start asusd. Any failure after the unmask
/// restores the previous asusd state, so asusd never ends up running beside G-Helper.
pub fn takeover(h: &mut dyn Host) -> anyhow::Result<()> {
    if h.asusd_masked() {
        h.set_marker(true)?;
        h.systemctl(&["unmask", "asusd"])?;
    }
    if let Err(e) = h.support_fix() {
        h.warn(&format!("lighting-zone fix skipped: {e:#}"));
    }
    if let Err(e) = h.systemctl(&["enable", "--now", "asusd"]) {
        return Err(match handback(h) {
            Ok(()) => e.context("asusd did not start; restored previous asusd state"),
            Err(r) => e.context(format!("asusd did not start and rollback failed: {r:#}")),
        });
    }
    Ok(())
}

pub fn handback(h: &mut dyn Host) -> anyhow::Result<()> {
    h.systemctl(&["disable", "--now", "asusd"])?;
    if h.marker_exists() {
        h.systemctl(&["mask", "asusd"])?;
        h.set_marker(false)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = r#"(
    (
        device_name: "G533Q",
        product_id: "",
        layout_name: "g533q-per-key",
        basic_modes: [Static],
        basic_zones: [],
        advanced_type: PerKey,
        power_zones: [Keyboard, Lightbar],
    ),
    (
        device_name: "G533Z",
        product_id: "",
        layout_name: "g533q-per-key",
        basic_modes: [Static, Breathe],
        basic_zones: [],
        advanced_type: PerKey,
        power_zones: [Keyboard],
    ),
    (
        device_name: "G614FM",
        product_id: "19b6",
        layout_name: "g634j-per-key",
        basic_modes: [Static],
        basic_zones: [],
        advanced_type: PerKey,
        power_zones: [Keyboard, Lightbar],
    ),
)"#;

    fn g533z() -> &'static Fix { fix_for_board("G533ZW").unwrap() }

    #[test]
    fn board_lookup() {
        assert_eq!(g533z().device, "G533Z");
        assert!(fix_for_board("G614FM").is_none());
    }

    #[test]
    fn patches_only_target_entry() {
        let out = patch_support(DB, g533z()).unwrap().unwrap();
        assert!(out.contains("device_name: \"G533Z\",\n        product_id: \"\",\n        layout_name: \"g533q-per-key\",\n        basic_modes: [Static, Breathe],\n        basic_zones: [],\n        advanced_type: PerKey,\n        power_zones: [Keyboard, Lightbar, Logo],"));
        assert_eq!(out.matches("power_zones: [Keyboard, Lightbar],").count(), 2, "neighbours untouched");
        assert_eq!(out.len(), DB.len() + ", Lightbar, Logo".len());
    }

    #[test]
    fn idempotent() {
        let once = patch_support(DB, g533z()).unwrap().unwrap();
        assert_eq!(patch_support(&once, g533z()).unwrap(), None);
    }

    #[test]
    fn keeps_existing_extra_zones() {
        let db = DB.replace("power_zones: [Keyboard],", "power_zones: [Keyboard, Lid],");
        let out = patch_support(&db, g533z()).unwrap().unwrap();
        assert!(out.contains("power_zones: [Keyboard, Lid, Lightbar, Logo],"));
    }

    #[test]
    fn missing_entry_is_error_not_write() {
        let db = DB.replace("G533Z", "G999X");
        assert!(patch_support(&db, g533z()).unwrap_err().contains("no entry for G533Z"));
    }

    #[test]
    fn entry_without_power_zones_is_error() {
        let db = DB.replace("        power_zones: [Keyboard],\n", "");
        let err = patch_support(&db, g533z()).unwrap_err();
        assert!(err.contains("power_zones"), "{err}");
    }

    #[test]
    fn stale_config_detection() {
        let only_kbd = "states: [ ( zone: Keyboard, boot: false, ), ]";
        assert!(aura_config_stale(only_kbd, g533z()));
        let full = "( zone: Keyboard, ) ( zone: Lightbar, ) ( zone: Logo, )";
        assert!(!aura_config_stale(full, g533z()));
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[derive(Default)]
    struct FakeHost {
        calls: Vec<String>,
        masked: bool,
        marker: bool,
        fail_on: Option<&'static str>,
        support_fix_fails: bool,
        warnings: Vec<String>,
    }

    impl Host for FakeHost {
        fn systemctl(&mut self, args: &[&str]) -> anyhow::Result<()> {
            let j = args.join(" ");
            self.calls.push(j.clone());
            if self.fail_on.is_some_and(|f| j.contains(f)) { anyhow::bail!("systemctl {j} failed"); }
            match args {
                ["unmask", _] => self.masked = false,
                ["mask", _] => self.masked = true,
                _ => {}
            }
            Ok(())
        }
        fn asusd_masked(&mut self) -> bool { self.masked }
        fn set_marker(&mut self, present: bool) -> anyhow::Result<()> { self.marker = present; Ok(()) }
        fn marker_exists(&mut self) -> bool { self.marker }
        fn support_fix(&mut self) -> anyhow::Result<()> {
            if self.support_fix_fails { anyhow::bail!("no entry for G533Z") } else { Ok(()) }
        }
        fn warn(&mut self, msg: &str) { self.warnings.push(msg.into()); }
    }

    #[test]
    fn takeover_unmasks_and_starts() {
        let mut h = FakeHost { masked: true, ..Default::default() };
        takeover(&mut h).unwrap();
        assert_eq!(h.calls, ["unmask asusd", "enable --now asusd"]);
        assert!(h.marker);
    }

    #[test]
    fn takeover_failure_restores_mask() {
        let mut h = FakeHost { masked: true, fail_on: Some("enable"), ..Default::default() };
        let err = takeover(&mut h).unwrap_err();
        assert!(format!("{err:#}").contains("restored previous asusd state"));
        assert_eq!(h.calls, ["unmask asusd", "enable --now asusd", "disable --now asusd", "mask asusd"]);
        assert!(h.masked);
        assert!(!h.marker);
    }

    #[test]
    fn support_fix_failure_does_not_block_takeover() {
        let mut h = FakeHost { support_fix_fails: true, ..Default::default() };
        takeover(&mut h).unwrap();
        assert_eq!(h.calls, ["enable --now asusd"]);
        assert!(h.warnings[0].contains("no entry for G533Z"));
    }

    #[test]
    fn handback_leaves_unmasked_if_it_was_not_masked() {
        let mut h = FakeHost::default();
        handback(&mut h).unwrap();
        assert_eq!(h.calls, ["disable --now asusd"]);
    }

    #[test]
    fn configs_set_aside_only_when_support_changed() {
        let fix = fix_for_board("G533ZW").unwrap();
        let cfgs = vec![("aura_19b6.ron".to_string(), "zone: Keyboard,".to_string())];
        assert!(stale_configs(false, &cfgs, fix).is_empty());
        assert_eq!(stale_configs(true, &cfgs, fix), ["aura_19b6.ron"]);
    }

    #[test]
    fn backup_never_overwrites() {
        let p = std::path::Path::new("/etc/asusd/aura_19b6.ron");
        assert_eq!(backup_path(p, |_| false).to_str(), Some("/etc/asusd/aura_19b6.ron.armoury-bak"));
        let taken = |q: &std::path::Path| q.to_str().unwrap().ends_with(".armoury-bak") || q.to_str().unwrap().ends_with(".armoury-bak.1");
        assert_eq!(backup_path(p, taken).to_str(), Some("/etc/asusd/aura_19b6.ron.armoury-bak.2"));
    }

    #[test]
    fn cached_package_path() {
        assert_eq!(
            cached_package("asusctl 6.4.0-2\n", "x86_64").as_deref(),
            Some("/var/cache/pacman/pkg/asusctl-6.4.0-2-x86_64.pkg.tar.zst")
        );
        assert_eq!(cached_package("", "x86_64"), None);
    }

    #[test]
    fn set_limits_maps_to_legacy_nodes() {
        let args: Vec<String> = ["pl1=120", "pl2=150", "nv_boost=25", "nv_temp=87", "cpu_boost=off"].map(String::from).to_vec();
        assert_eq!(parse_limits(&args).unwrap(), vec![
            ("/sys/devices/platform/asus-nb-wmi/ppt_pl1_spl", "120".to_string()),
            ("/sys/devices/platform/asus-nb-wmi/ppt_pl2_sppt", "150".to_string()),
            ("/sys/devices/platform/asus-nb-wmi/nv_dynamic_boost", "25".to_string()),
            ("/sys/devices/platform/asus-nb-wmi/nv_temp_target", "87".to_string()),
            ("/sys/devices/system/cpu/intel_pstate/no_turbo", "1".to_string()),
        ]);
    }

    #[test]
    fn set_limits_rejects_bad_input() {
        let bad = |a: &str| parse_limits(&[a.to_string()]).unwrap_err();
        assert!(bad("pl1=900").contains("pl1"));
        assert!(bad("pl1=abc").contains("pl1"));
        assert!(bad("fan=3").contains("unknown"));
        assert!(bad("cpu_boost=maybe").contains("on|off"));
        assert!(bad("pl1").contains("KEY=VALUE"));
        assert!(parse_limits(&[]).unwrap_err().contains("nothing"));
    }

    #[test]
    fn set_limits_caps_match_model_ranges() {
        assert!(parse_limits(&["pl1=200".to_string()]).is_err());
        assert!(parse_limits(&["pl1=175".to_string()]).is_ok());
        assert!(parse_limits(&["nv_boost=30".to_string()]).is_err());
        assert!(parse_limits(&["nv_temp=70".to_string()]).is_err());
    }

    #[test]
    fn write_all_attempts_every_node() {
        let pairs = vec![("a", "1".to_string()), ("b", "2".to_string()), ("c", "3".to_string())];
        let mut seen = Vec::new();
        let errs = write_all(&pairs, |node, _| { seen.push(node.to_string()); if node == "b" { Err("boom".into()) } else { Ok(()) } });
        assert_eq!(seen, ["a", "b", "c"]);
        assert_eq!(errs, ["b: boom"]);
    }

    #[test]
    fn active_flag_under_home() {
        assert_eq!(active_flag(std::path::Path::new("/home/u")), std::path::Path::new("/home/u/.local/state/omarchy-armoury/active"));
    }
}
