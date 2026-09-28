pub const SUPPORT_FILE: &str = "/usr/share/asusd/aura_support.ron";
pub const AURA_CFG_DIR: &str = "/etc/asusd";
pub const STATE_DIR: &str = "/var/lib/omarchy-armoury";
pub const BOARD_NAME: &str = "/sys/class/dmi/id/board_name";

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
