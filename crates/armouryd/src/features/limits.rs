use armoury_proto::ModeSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit { Pl1, Pl2, NvBoost, NvTemp }

impl Limit {
    pub const ALL: [Limit; 4] = [Limit::Pl1, Limit::Pl2, Limit::NvBoost, Limit::NvTemp];
    pub fn attr(self) -> &'static str {
        match self { Self::Pl1 => "ppt_pl1_spl", Self::Pl2 => "ppt_pl2_sppt", Self::NvBoost => "nv_dynamic_boost", Self::NvTemp => "nv_temp_target" }
    }
    /// Used when firmware reports no range (g-helper-linux defaults, non-HX Intel).
    pub fn fallback(self) -> (i32, i32) {
        match self { Self::Pl1 | Self::Pl2 => (5, 150), Self::NvBoost => (5, 25), Self::NvTemp => (75, 87) }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Pl1 => "PL1", Self::Pl2 => "PL2", Self::NvBoost => "Dynamic Boost", Self::NvTemp => "GPU temp target" }
    }
    pub fn value(self, s: &ModeSettings) -> Option<i32> {
        match self { Self::Pl1 => s.pl1, Self::Pl2 => s.pl2, Self::NvBoost => s.nv_boost, Self::NvTemp => s.nv_temp }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds { pub min: i32, pub max: i32 }

pub fn bounds(limit: Limit, reported: Option<(i32, i32)>) -> Bounds {
    match reported {
        Some((min, max)) if 0 < min && min <= max => Bounds { min, max },
        _ => { let (min, max) = limit.fallback(); Bounds { min, max } }
    }
}

pub fn validate(s: &ModeSettings, b: impl Fn(Limit) -> Bounds) -> Result<(), String> {
    for l in Limit::ALL {
        if let Some(v) = l.value(s) {
            let Bounds { min, max } = b(l);
            if v < min || v > max { return Err(format!("{} must be {min}–{max}", l.label())); }
        }
    }
    if let (Some(p1), Some(p2)) = (s.pl1, s.pl2) {
        if p1 > p2 { return Err("PL1 must not exceed PL2".into()); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(l: Limit) -> Bounds { bounds(l, None) }

    #[test]
    fn firmware_bounds_used_only_when_sane() {
        assert_eq!(bounds(Limit::Pl1, Some((-1, -1))), Bounds { min: 5, max: 150 });
        assert_eq!(bounds(Limit::Pl1, Some((15, 90))), Bounds { min: 15, max: 90 });
        assert_eq!(bounds(Limit::NvTemp, None), Bounds { min: 75, max: 87 });
    }

    #[test]
    fn accepts_in_range() {
        let s = ModeSettings { pl1: Some(120), pl2: Some(150), nv_boost: Some(25), nv_temp: Some(87), ..Default::default() };
        assert!(validate(&s, fb).is_ok());
        assert!(validate(&ModeSettings::default(), fb).is_ok());
    }

    #[test]
    fn rejects_out_of_range_and_inverted() {
        let e = validate(&ModeSettings { pl1: Some(200), ..Default::default() }, fb).unwrap_err();
        assert!(e.contains("PL1") && e.contains("5–150"), "{e}");
        let e = validate(&ModeSettings { pl1: Some(140), pl2: Some(100), ..Default::default() }, fb).unwrap_err();
        assert!(e.contains("PL1") && e.contains("PL2"), "{e}");
        assert!(validate(&ModeSettings { nv_temp: Some(90), ..Default::default() }, fb).is_err());
    }
}
