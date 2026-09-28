use armoury_proto::{Fan, FanCurve};

/// asusd's wire form: (fan name, pwm 0–255, temperature °C, enabled).
pub type RawCurve = (String, [u8; 8], [u8; 8], bool);

pub fn pct_to_pwm(p: u8) -> u8 { ((p.min(100) as u32 * 255 + 50) / 100) as u8 }
pub fn pwm_to_pct(v: u8) -> u8 { ((v as u32 * 100 + 127) / 255) as u8 }

pub fn from_raw(raw: &RawCurve) -> Option<FanCurve> {
    Some(FanCurve { fan: Fan::from_asusd(&raw.0)?, temps: raw.2, percent: raw.1.map(pwm_to_pct), enabled: raw.3 })
}

pub fn to_raw(c: &FanCurve) -> RawCurve {
    (c.fan.asusd_name().to_string(), c.percent.map(pct_to_pwm), c.temps, c.enabled)
}

pub fn validate(c: &FanCurve) -> Result<(), String> {
    if c.percent.iter().any(|&p| p > 100) { return Err("fan speed must be 0–100%".into()); }
    if c.temps.iter().any(|&t| t > 120) { return Err("temperature must be at most 120 °C".into()); }
    if c.temps.windows(2).any(|w| w[1] < w[0]) { return Err("temperature points must not decrease".into()); }
    if c.percent.windows(2).any(|w| w[1] < w[0]) { return Err("fan speed points must not decrease".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use armoury_proto::Fan;

    fn curve() -> FanCurve {
        FanCurve { fan: Fan::Cpu, temps: [40, 50, 60, 65, 70, 75, 80, 90], percent: [0, 5, 20, 30, 45, 60, 80, 100], enabled: true }
    }

    #[test]
    fn percent_pwm_round_trip() {
        assert_eq!(pct_to_pwm(100), 255);
        assert_eq!(pct_to_pwm(0), 0);
        assert_eq!(pwm_to_pct(255), 100);
        assert_eq!(pwm_to_pct(128), 50);
        for p in 0..=100u8 { assert_eq!(pwm_to_pct(pct_to_pwm(p)), p); }
    }

    #[test]
    fn raw_conversion_matches_asusd_data() {
        // performance CPU curve read from this machine's asusd
        let raw: RawCurve = ("CPU".into(), [33, 73, 91, 104, 122, 170, 222, 255], [51, 55, 59, 63, 67, 71, 75, 97], true);
        let c = from_raw(&raw).unwrap();
        assert_eq!(c.fan, Fan::Cpu);
        assert_eq!(c.percent, [13, 29, 36, 41, 48, 67, 87, 100]);
        assert_eq!(c.temps[7], 97);
        assert_eq!(to_raw(&curve()).0, "CPU");
        assert!(from_raw(&("XYZ".into(), [0; 8], [0; 8], false)).is_none());
    }

    #[test]
    fn validate_accepts_monotonic() {
        assert!(validate(&curve()).is_ok());
    }

    #[test]
    fn validate_rejects_bad_curves() {
        let mut c = curve();
        c.percent[3] = 10;
        assert!(validate(&c).unwrap_err().contains("fan speed"));
        let mut c = curve();
        c.temps[5] = 60;
        assert!(validate(&c).unwrap_err().contains("temperature"));
        let mut c = curve();
        c.percent[7] = 120;
        assert!(validate(&c).unwrap_err().contains("100"));
        let mut c = curve();
        c.temps[7] = 130;
        assert!(validate(&c).unwrap_err().contains("120"));
    }
}
