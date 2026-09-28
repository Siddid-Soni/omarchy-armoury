use armoury_proto::{AuraEffect, AuraMode, AuraZone, Direction, Speed, ZonePower};

/// asusd LedModeData: (mode, zone, colour1, colour2, speed, direction).
pub type RawMode = (u32, u32, (u8, u8, u8), (u8, u8, u8), String, String);
/// asusd LedPower: a struct holding (zone, boot, awake, sleep, shutdown) per zone.
pub type RawPower = (Vec<(u32, bool, bool, bool, bool)>,);

fn speed_str(s: Speed) -> &'static str {
    match s { Speed::Low => "Low", Speed::Med => "Med", Speed::High => "High" }
}

fn dir_str(d: Direction) -> &'static str {
    match d { Direction::Right => "Right", Direction::Left => "Left", Direction::Up => "Up", Direction::Down => "Down" }
}

pub fn effect_from_raw(r: &RawMode) -> Option<AuraEffect> {
    let speed = match r.4.as_str() { "Low" => Speed::Low, "High" => Speed::High, _ => Speed::Med };
    let direction = match r.5.as_str() { "Left" => Direction::Left, "Up" => Direction::Up, "Down" => Direction::Down, _ => Direction::Right };
    Some(AuraEffect {
        mode: AuraMode::from_code(r.0)?,
        colour1: [r.2.0, r.2.1, r.2.2],
        colour2: [r.3.0, r.3.1, r.3.2],
        speed,
        direction,
    })
}

/// Zone 0 = whole keyboard (per-zone keyboards are not used on this model).
pub fn effect_to_raw(e: &AuraEffect) -> RawMode {
    let [a, b, c] = e.colour1;
    let [d, f, g] = e.colour2;
    (e.mode.code(), 0, (a, b, c), (d, f, g), speed_str(e.speed).into(), dir_str(e.direction).into())
}

pub fn power_from_raw(r: &RawPower) -> Vec<ZonePower> {
    r.0.iter().filter_map(|&(z, boot, awake, sleep, shutdown)| {
        Some(ZonePower { zone: AuraZone::from_code(z)?, boot, awake, sleep, shutdown })
    }).collect()
}

/// Replaces one zone's states, keeping every other zone as asusd reported it.
pub fn set_zone(r: &RawPower, z: &ZonePower) -> RawPower {
    let entry = (z.zone.code(), z.boot, z.awake, z.sleep, z.shutdown);
    let mut v: Vec<_> = r.0.iter().copied().filter(|e| e.0 != entry.0).collect();
    match r.0.iter().position(|e| e.0 == entry.0) {
        Some(i) => v.insert(i, entry),
        None => v.push(entry),
    }
    (v,)
}

pub fn validate_effect(e: &AuraEffect, supported: &[AuraMode]) -> Result<(), String> {
    if !supported.contains(&e.mode) { return Err(format!("{:?} is not supported by this keyboard", e.mode)); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // captured from this machine's asusd
    fn raw_mode() -> RawMode { (0, 0, (166, 0, 0), (0, 0, 0), "Med".into(), "Right".into()) }
    fn raw_power() -> RawPower {
        (vec![(1, true, true, false, false), (2, true, true, false, false), (0, true, true, false, false)],)
    }

    #[test]
    fn effect_round_trip() {
        let e = effect_from_raw(&raw_mode()).unwrap();
        assert_eq!(e, AuraEffect { mode: AuraMode::Static, colour1: [166, 0, 0], colour2: [0, 0, 0], speed: Speed::Med, direction: Direction::Right });
        assert_eq!(effect_to_raw(&e), raw_mode());
        let wave = AuraEffect { mode: AuraMode::RainbowWave, speed: Speed::High, direction: Direction::Left, ..e };
        assert_eq!(effect_to_raw(&wave), (3, 0, (166, 0, 0), (0, 0, 0), "High".into(), "Left".into()));
        assert!(effect_from_raw(&(9, 0, (0, 0, 0), (0, 0, 0), "Med".into(), "Right".into())).is_none());
    }

    #[test]
    fn power_conversion_and_single_zone_update() {
        let zones = power_from_raw(&raw_power());
        assert_eq!(zones.len(), 3);
        assert_eq!(zones[2], ZonePower { zone: AuraZone::Logo, boot: true, awake: true, sleep: false, shutdown: false });
        let off = ZonePower { zone: AuraZone::Logo, boot: false, awake: false, sleep: false, shutdown: false };
        let new = set_zone(&raw_power(), &off);
        assert_eq!(new.0[2], (0, false, false, false, false));
        assert_eq!(new.0[0], raw_power().0[0], "other zones untouched");
        assert_eq!(new.0.len(), 3);
    }

    #[test]
    fn unsupported_mode_rejected() {
        let e = AuraEffect { mode: AuraMode::Comet, colour1: [0; 3], colour2: [0; 3], speed: Speed::Med, direction: Direction::Right };
        assert!(validate_effect(&e, &[AuraMode::Static]).unwrap_err().contains("not supported"));
        assert!(validate_effect(&e, &[AuraMode::Comet]).is_ok());
    }
}
