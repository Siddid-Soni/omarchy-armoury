# Plan 2 — Performance modes, fan curves, power limits, CPU boost, monitor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** armouryd can read and change performance mode, per-mode fan curves, per-mode power limits (PL1/PL2, NVIDIA Dynamic Boost + temp target), per-mode EPP and CPU boost; it re-applies a mode's settings whenever the mode changes and optionally every N seconds; and it reports CPU temperature, fan RPM and power draw.

**Architecture:** A new `Asusd` hardware trait wraps asusd's D-Bus (`xyz.ljones.Platform`, `xyz.ljones.FanCurves`, `xyz.ljones.AsusArmoury`). Pure logic (fan-curve conversion/validation, power-limit bounds) lives in `features/`. Per-mode settings that the firmware cannot report back are stored in `~/.config/omarchy-armoury/config.toml`. Readings come from sysfs so they work in observe mode. CPU boost is a new `armoury-root cpu-boost` command.

**Tech Stack:** as Plan 1, plus `toml = "1"`.

**Spec:** `docs/superpowers/specs/2026-09-29-omarchy-armoury-design.md` (§3.2, §3.5, §4.1, §9 step 2). Undervolt, NVIDIA clock offsets/locks and GPU status (also §4.1) are Plan 2b.

## Global Constraints

- Writes only in active mode (`Control::require_active`); reads work in observe mode.
- Every asusd call: `with_retry` (3 s timeout, one retry).
- armoury-root: fixed allowlist, arguments re-validated (`cpu-boost` takes only `on`/`off`).
- Only controls whose attributes exist are offered (spec §4.1); manual fixed-RPM fan is not exposed by this firmware (`asus` hwmon has only `pwm*_enable`), so it is not built.
- Config written only after a successful apply (spec §3.5).

**Verified on the reference machine (asusd 6.4.0, taken via a read-only takeover):**
- `xyz.ljones.Platform` at `/xyz/ljones`: `PlatformProfile u` (0 Balanced, 1 Performance, 2 Quiet, 3 LowPower, 4 Custom), `PlatformProfileChoices au`, `EnablePptGroup b`, `ProfileQuietEpp/ProfileBalancedEpp/ProfilePerformanceEpp u` (0 Default, 1 Performance, 2 BalancePerformance, 3 BalancePower, 4 Power), `NextPlatformProfile()`.
- `xyz.ljones.FanCurves` at `/xyz/ljones`: `FanCurveData(u) -> a(s(yyyyyyyy)(yyyyyyyy)b)` (fan name `"CPU"|"GPU"|"MID"`, pwm 0–255, temp °C, enabled); `SetFanCurve(u, (s(yyyyyyyy)(yyyyyyyy)b))`, `SetCurvesToDefaults(u)`, `SetProfileFanCurveEnabled(u, s, b)`.
- `xyz.ljones.AsusArmoury` at `/xyz/ljones/asus_armoury/<attr>`: `CurrentValue i` (writable; **read fails** for `ppt_pl1_spl`, `ppt_pl2_sppt`, `nv_dynamic_boost`, `nv_temp_target`), `MinValue`/`MaxValue i` (**-1** on this machine). asusd only writes PPT values to hardware while `EnablePptGroup` is true.
- sysfs: `platform_profile_choices` = `quiet balanced performance`; hwmon `coretemp` `temp1` = `Package id 0` (m°C); hwmon `asus` `fan1_input`/`fan2_input` (RPM, labels `cpu_fan`/`gpu_fan`); `BAT0/power_now` (µW); `intel_pstate/no_turbo`.
- Fallback ranges when firmware reports none (from g-helper-linux for non-HX Intel): PL1/PL2 5–150 W, Dynamic Boost 5–25 W, temp target 75–87 °C.

## Review Focus

1. **Mode changed outside armouryd (Fn+F5, asusd AC/battery auto-switch)** → that mode's stored limits/EPP/boost are applied within one poll (2 s). Pinned in Task 8 (`external_profile_change_applies_mode`).
2. **Out-of-range or inverted input (PL1 > PL2, 200 W, fan curve going down, 120 %)** → rejected with a clear message, nothing written, config unchanged. Pinned in Tasks 2, 3, 8.
3. **asusd not running / write fails mid-apply** → the error is returned, remaining settings still attempted, config not saved for the failed request. Pinned in Task 7 (`apply_continues_after_error`) and Task 8 (`failed_set_does_not_save`).
4. **Corrupt or hand-edited config.toml** → daemon starts with defaults and reports the parse error instead of crashing. Pinned in Task 4 (`corrupt_config_falls_back`).
5. **Observe mode** → every write request refused, poll never applies anything. Pinned in Task 8 (`observe_mode_never_applies`).

---

## File Structure

```
crates/proto/src/lib.rs                  + Profile, Epp, Fan, FanCurve, ModeSettings, PerfState, new Requests
crates/armouryd/src/features/mod.rs      module wiring
crates/armouryd/src/features/fan.rs      pwm<->percent, curve validation
crates/armouryd/src/features/limits.rs   bounds + validation of ModeSettings
crates/armouryd/src/features/perf.rs     apply_mode()
crates/armouryd/src/config.rs            Config load/save (toml)
crates/armouryd/src/hw/asusd.rs          Asusd trait impl over zbus
crates/armouryd/src/hw/mod.rs            + Asusd trait
crates/armouryd/src/hw/fake.rs           + FakeAsusd
crates/armouryd/src/hw/sysfs.rs          + hwmon helpers
crates/armouryd/src/state.rs             + perf readings
crates/armouryd/src/ipc.rs               + request handlers, auto-apply, re-apply timer
crates/armouryd/src/main.rs              wire asusd + config
crates/armoury-root/src/{lib,main}.rs    + cpu-boost
crates/armoury/src/main.rs               + profile / fan / mode subcommands
```

---

### Task 1: Protocol types

**Files:** Modify `crates/proto/src/lib.rs`

**Interfaces — Produces:**
- `Profile { Quiet, Balanced, Performance }` (serde lowercase; `Ord`), `Profile::from_asusd(u32) -> Option<Profile>` (2/0/1), `Profile::to_asusd(self) -> u32`, `Profile::from_sysfs(&str) -> Option<Profile>` (`quiet|balanced|performance`), `Profile::sysfs(self) -> &'static str`.
- `Epp { Default, Performance, BalancePerformance, BalancePower, Power }` (serde snake_case), `Epp::to_asusd(self) -> u32` (0–4), `Epp::from_asusd(u32) -> Option<Epp>`.
- `Fan { Cpu, Gpu, Mid }` (serde lowercase), `Fan::asusd_name(self) -> &'static str` (`"CPU"` …), `Fan::from_asusd(&str) -> Option<Fan>`.
- `FanCurve { fan: Fan, temps: [u8; 8], percent: [u8; 8], enabled: bool }`.
- `ModeSettings { pl1: Option<i32>, pl2: Option<i32>, nv_boost: Option<i32>, nv_temp: Option<i32>, epp: Option<Epp>, cpu_boost: Option<bool> }` (`Default`, all `None` = leave untouched; serde skips `None`).
- `PerfState { profile: Option<Profile>, choices: Vec<Profile>, cpu_temp_c: Option<f32>, cpu_fan_rpm: Option<u32>, gpu_fan_rpm: Option<u32>, power_draw_w: Option<f32>, cpu_boost: Option<bool> }`; `Snapshot.perf: PerfState`.
- `Request` gains: `SetProfile { profile: Profile }`, `NextProfile`, `FanCurves { profile: Profile }`, `SetFanCurve { profile: Profile, curve: FanCurve }`, `ResetFanCurves { profile: Profile }`, `ModeSettings { profile: Profile }`, `SetModeSettings { profile: Profile, settings: ModeSettings }`.

- [ ] **Step 1: Failing tests** (append inside `mod tests`)

```rust
    #[test]
    fn profile_codes() {
        assert_eq!(Profile::from_asusd(2), Some(Profile::Quiet));
        assert_eq!(Profile::from_asusd(3), None);
        assert_eq!(Profile::Performance.to_asusd(), 1);
        assert_eq!(Profile::from_sysfs("balanced"), Some(Profile::Balanced));
        assert_eq!(Profile::Quiet.sysfs(), "quiet");
        assert_eq!(Epp::BalancePower.to_asusd(), 3);
        assert_eq!(Fan::from_asusd("GPU"), Some(Fan::Gpu));
        assert_eq!(Fan::Cpu.asusd_name(), "CPU");
    }

    #[test]
    fn new_requests_wire_format() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_profile","profile":"quiet"}"#).unwrap();
        assert_eq!(r, Request::SetProfile { profile: Profile::Quiet });
        let r: Request = serde_json::from_str(
            r#"{"cmd":"set_mode_settings","profile":"performance","settings":{"pl1":120,"epp":"balance_power"}}"#,
        ).unwrap();
        assert_eq!(r, Request::SetModeSettings {
            profile: Profile::Performance,
            settings: ModeSettings { pl1: Some(120), epp: Some(Epp::BalancePower), ..Default::default() },
        });
        let v = serde_json::to_value(ModeSettings { pl2: Some(150), ..Default::default() }).unwrap();
        assert_eq!(v, serde_json::json!({"pl2": 150}));
    }
```

- [ ] **Step 2: Run** `cargo test -p armoury-proto` — Expected: FAIL, `cannot find type Profile`.

- [ ] **Step 3: Implement** (add after `GpuPower` impl)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile { Quiet, Balanced, Performance }

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Quiet, Profile::Balanced, Profile::Performance];
    pub fn from_asusd(v: u32) -> Option<Self> {
        match v { 0 => Some(Self::Balanced), 1 => Some(Self::Performance), 2 => Some(Self::Quiet), _ => None }
    }
    pub fn to_asusd(self) -> u32 {
        match self { Self::Balanced => 0, Self::Performance => 1, Self::Quiet => 2 }
    }
    pub fn from_sysfs(s: &str) -> Option<Self> {
        match s { "quiet" => Some(Self::Quiet), "balanced" => Some(Self::Balanced), "performance" => Some(Self::Performance), _ => None }
    }
    pub fn sysfs(self) -> &'static str {
        match self { Self::Quiet => "quiet", Self::Balanced => "balanced", Self::Performance => "performance" }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Epp { Default, Performance, BalancePerformance, BalancePower, Power }

impl Epp {
    pub fn to_asusd(self) -> u32 { self as u32 }
    pub fn from_asusd(v: u32) -> Option<Self> {
        [Self::Default, Self::Performance, Self::BalancePerformance, Self::BalancePower, Self::Power].get(v as usize).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fan { Cpu, Gpu, Mid }

impl Fan {
    pub fn asusd_name(self) -> &'static str {
        match self { Self::Cpu => "CPU", Self::Gpu => "GPU", Self::Mid => "MID" }
    }
    pub fn from_asusd(s: &str) -> Option<Self> {
        match s { "CPU" => Some(Self::Cpu), "GPU" => Some(Self::Gpu), "MID" => Some(Self::Mid), _ => None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanCurve {
    pub fan: Fan,
    pub temps: [u8; 8],
    pub percent: [u8; 8],
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pl1: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pl2: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nv_boost: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nv_temp: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epp: Option<Epp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_boost: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerfState {
    pub profile: Option<Profile>,
    pub choices: Vec<Profile>,
    pub cpu_temp_c: Option<f32>,
    pub cpu_fan_rpm: Option<u32>,
    pub gpu_fan_rpm: Option<u32>,
    pub power_draw_w: Option<f32>,
    pub cpu_boost: Option<bool>,
}
```

Add `pub perf: PerfState,` to `Snapshot` (after `battery`). Replace `Request` with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Subscribe,
    Takeover,
    Handback,
    SetProfile { profile: Profile },
    NextProfile,
    FanCurves { profile: Profile },
    SetFanCurve { profile: Profile, curve: FanCurve },
    ResetFanCurves { profile: Profile },
    ModeSettings { profile: Profile },
    SetModeSettings { profile: Profile, settings: ModeSettings },
}
```

(`PerfState` holds `f32`, so `Snapshot` keeps `PartialEq` only — it already has no `Eq`.)

- [ ] **Step 4: Run** `cargo test -p armoury-proto` — Expected: 6 passed. Then `cargo build --workspace` — Expected: armouryd fails to compile only on the non-exhaustive `match req` in `ipc.rs`; add a temporary arm `_ => Response::err("not implemented")` there (replaced in Task 8) and rebuild clean.

- [ ] **Step 5: Commit** `git commit -am "feat(proto): performance mode, fan curve and mode-settings types"`

---

### Task 2: Fan-curve conversion and validation

**Files:** Create `crates/armouryd/src/features/mod.rs`, `crates/armouryd/src/features/fan.rs`; modify `lib.rs` (`pub mod features;`)

**Interfaces — Produces:** `pub type RawCurve = (String, [u8; 8], [u8; 8], bool)` (fan name, pwm, temps, enabled); `fn from_raw(raw: &RawCurve) -> Option<FanCurve>`; `fn to_raw(c: &FanCurve) -> RawCurve`; `fn validate(c: &FanCurve) -> Result<(), String>`; `fn pct_to_pwm(u8) -> u8`, `fn pwm_to_pct(u8) -> u8` (rounded).

- [ ] **Step 1: Failing tests** — `features/mod.rs`: `pub mod fan;` ; `features/fan.rs`:

```rust
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
```

- [ ] **Step 2: Run** `cargo test -p armouryd features::fan` — Expected: FAIL, unresolved names.

- [ ] **Step 3: Implement** (top of `fan.rs`)

```rust
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
```

- [ ] **Step 4: Run** `cargo test -p armouryd features::fan` — Expected: 4 passed.
- [ ] **Step 5: Commit** `git add -A crates/armouryd && git commit -m "feat(armouryd): fan-curve conversion and validation"`

---

### Task 3: Power-limit bounds and mode-settings validation

**Files:** Create `crates/armouryd/src/features/limits.rs`; `features/mod.rs` add `pub mod limits;`

**Interfaces — Produces:** `enum Limit { Pl1, Pl2, NvBoost, NvTemp }` with `Limit::attr(self) -> &'static str` (`ppt_pl1_spl`, `ppt_pl2_sppt`, `nv_dynamic_boost`, `nv_temp_target`) and `Limit::fallback(self) -> (i32, i32)` ((5,150),(5,150),(5,25),(75,87)); `struct Bounds { pub min: i32, pub max: i32 }`, `fn bounds(limit, reported: Option<(i32, i32)>) -> Bounds` (use reported only if `0 < min <= max`); `fn validate(s: &ModeSettings, b: impl Fn(Limit) -> Bounds) -> Result<(), String>`.

- [ ] **Step 1: Failing tests**

```rust
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
```

- [ ] **Step 2: Run** `cargo test -p armouryd features::limits` — Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
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
    fn label(self) -> &'static str {
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
```

- [ ] **Step 4: Run** — Expected: 3 passed.
- [ ] **Step 5: Commit** `git add -A crates/armouryd && git commit -m "feat(armouryd): power-limit bounds and validation"`

---

### Task 4: Config file

**Files:** Create `crates/armouryd/src/config.rs`; `lib.rs` add `pub mod config;`; `crates/armouryd/Cargo.toml` add `toml = "1"`.

**Interfaces — Produces:** `struct Config { pub modes: BTreeMap<Profile, ModeSettings>, pub reapply_power_secs: u32 }` (`Default`, serde default); `Config::load(path: &Path) -> (Config, Option<String>)` (missing file → default + `None`; parse error → default + `Some(error)`); `Config::save(&self, path: &Path) -> io::Result<()>` (atomic: write `.tmp`, rename; creates parent dir); `Config::mode(&self, p: Profile) -> ModeSettings`; `fn config_path(home: &Path) -> PathBuf` = `home/.config/omarchy-armoury/config.toml`.

- [ ] **Step 1: Failing tests**

```rust
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
```

- [ ] **Step 2: Run** `cargo test -p armouryd config` — Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
use armoury_proto::{ModeSettings, Profile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub reapply_power_secs: u32,
    pub modes: BTreeMap<Profile, ModeSettings>,
}

pub fn config_path(home: &Path) -> PathBuf {
    home.join(".config/omarchy-armoury/config.toml")
}

impl Config {
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
```

(Field order: `reapply_power_secs` before `modes` so TOML puts the scalar before the tables.)

- [ ] **Step 4: Run** — Expected: 4 passed.
- [ ] **Step 5: Commit** `git add -A crates/armouryd Cargo.lock && git commit -m "feat(armouryd): toml config with per-mode settings"`

---

### Task 5: Asusd hardware trait, zbus client, fake

**Files:** Create `crates/armouryd/src/hw/asusd.rs`; modify `hw/mod.rs`, `hw/fake.rs`.

**Interfaces — Produces:**
```rust
#[async_trait::async_trait]
pub trait Asusd: Send + Sync {
    async fn set_profile(&self, p: u32) -> anyhow::Result<()>;
    async fn next_profile(&self) -> anyhow::Result<()>;
    async fn fan_curves(&self, profile: u32) -> anyhow::Result<Vec<RawCurve>>;
    async fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> anyhow::Result<()>;
    async fn reset_fan_curves(&self, profile: u32) -> anyhow::Result<()>;
    async fn set_ppt_group(&self, enabled: bool) -> anyhow::Result<()>;
    async fn armoury_range(&self, attr: &str) -> anyhow::Result<(i32, i32)>;
    async fn armoury_set(&self, attr: &str, value: i32) -> anyhow::Result<()>;
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()>;
}
```
`AsusdClient::new().await -> anyhow::Result<AsusdClient>`. `FakeAsusd { calls: Mutex<Vec<String>>, curves: Mutex<HashMap<u32, Vec<RawCurve>>>, fail_on: Mutex<Option<String>> }` recording calls as e.g. `"set_profile 1"`, `"armoury_set ppt_pl1_spl 120"`, `"set_ppt_group true"`, `"set_profile_epp 1 1"`, `"set_fan_curve 1 CPU"`, `"reset_fan_curves 1"`, `"next_profile"`; `armoury_range` returns `(-1, -1)`.

- [ ] **Step 1: Failing test** (append to `hw/fake.rs` tests)

```rust
    #[tokio::test]
    async fn fake_asusd_records_and_fails() {
        use crate::hw::Asusd;
        let a = FakeAsusd::default();
        a.armoury_set("ppt_pl1_spl", 120).await.unwrap();
        *a.fail_on.lock().unwrap() = Some("pl2".into());
        assert!(a.armoury_set("ppt_pl2_sppt", 150).await.is_err());
        a.curves.lock().unwrap().insert(1, vec![("CPU".into(), [0; 8], [0; 8], true)]);
        assert_eq!(a.fan_curves(1).await.unwrap().len(), 1);
        assert_eq!(a.armoury_range("x").await.unwrap(), (-1, -1));
        assert_eq!(a.calls.lock().unwrap()[0], "armoury_set ppt_pl1_spl 120");
    }
```

- [ ] **Step 2: Run** `cargo test -p armouryd hw::fake` — Expected: FAIL, `FakeAsusd` not found.

- [ ] **Step 3: Implement.** In `hw/mod.rs` add `pub mod asusd;` and the trait above (with `use crate::features::fan::RawCurve;`). `hw/asusd.rs`:

```rust
use super::Asusd;
use crate::features::fan::RawCurve;

#[zbus::proxy(interface = "xyz.ljones.Platform", default_service = "xyz.ljones.Asusd", default_path = "/xyz/ljones")]
trait Platform {
    fn next_platform_profile(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn platform_profile(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_platform_profile(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn enable_ppt_group(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_enable_ppt_group(&self, value: bool) -> zbus::Result<()>;
    #[zbus(property)]
    fn profile_quiet_epp(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_profile_quiet_epp(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn profile_balanced_epp(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_profile_balanced_epp(&self, value: u32) -> zbus::Result<()>;
    #[zbus(property)]
    fn profile_performance_epp(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn set_profile_performance_epp(&self, value: u32) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "xyz.ljones.FanCurves", default_service = "xyz.ljones.Asusd", default_path = "/xyz/ljones")]
trait FanCurves {
    fn fan_curve_data(&self, profile: u32) -> zbus::Result<Vec<RawCurve>>;
    fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> zbus::Result<()>;
    fn set_curves_to_defaults(&self, profile: u32) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "xyz.ljones.AsusArmoury", default_service = "xyz.ljones.Asusd")]
trait Armoury {
    #[zbus(property)]
    fn current_value(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn set_current_value(&self, value: i32) -> zbus::Result<()>;
    #[zbus(property)]
    fn min_value(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn max_value(&self) -> zbus::Result<i32>;
}

pub struct AsusdClient {
    conn: zbus::Connection,
}

impl AsusdClient {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self { conn: zbus::Connection::system().await? })
    }
    async fn platform(&self) -> zbus::Result<PlatformProxy<'_>> { PlatformProxy::builder(&self.conn).cache_properties(zbus::proxy::CacheProperties::No).build().await }
    async fn fans(&self) -> zbus::Result<FanCurvesProxy<'_>> { FanCurvesProxy::new(&self.conn).await }
    async fn armoury(&self, attr: &str) -> anyhow::Result<ArmouryProxy<'_>> {
        anyhow::ensure!(attr.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'), "bad attribute {attr}");
        Ok(ArmouryProxy::builder(&self.conn)
            .path(format!("/xyz/ljones/asus_armoury/{attr}"))?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build().await?)
    }
}

#[async_trait::async_trait]
impl Asusd for AsusdClient {
    async fn set_profile(&self, p: u32) -> anyhow::Result<()> { Ok(self.platform().await?.set_platform_profile(p).await?) }
    async fn next_profile(&self) -> anyhow::Result<()> { Ok(self.platform().await?.next_platform_profile().await?) }
    async fn fan_curves(&self, profile: u32) -> anyhow::Result<Vec<RawCurve>> { Ok(self.fans().await?.fan_curve_data(profile).await?) }
    async fn set_fan_curve(&self, profile: u32, curve: RawCurve) -> anyhow::Result<()> { Ok(self.fans().await?.set_fan_curve(profile, curve).await?) }
    async fn reset_fan_curves(&self, profile: u32) -> anyhow::Result<()> { Ok(self.fans().await?.set_curves_to_defaults(profile).await?) }
    async fn set_ppt_group(&self, enabled: bool) -> anyhow::Result<()> { Ok(self.platform().await?.set_enable_ppt_group(enabled).await?) }
    async fn armoury_range(&self, attr: &str) -> anyhow::Result<(i32, i32)> {
        let a = self.armoury(attr).await?;
        Ok((a.min_value().await?, a.max_value().await?))
    }
    async fn armoury_set(&self, attr: &str, value: i32) -> anyhow::Result<()> { Ok(self.armoury(attr).await?.set_current_value(value).await?) }
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()> {
        let p = self.platform().await?;
        Ok(match profile {
            2 => p.set_profile_quiet_epp(epp).await?,
            0 => p.set_profile_balanced_epp(epp).await?,
            1 => p.set_profile_performance_epp(epp).await?,
            _ => anyhow::bail!("no EPP slot for profile {profile}"),
        })
    }
}
```

`hw/fake.rs` additions:

```rust
use super::Asusd;
use crate::features::fan::RawCurve;

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
    async fn set_ppt_group(&self, enabled: bool) -> anyhow::Result<()> { self.record(format!("set_ppt_group {enabled}")) }
    async fn armoury_range(&self, _attr: &str) -> anyhow::Result<(i32, i32)> { Ok((-1, -1)) }
    async fn armoury_set(&self, attr: &str, value: i32) -> anyhow::Result<()> { self.record(format!("armoury_set {attr} {value}")) }
    async fn set_profile_epp(&self, profile: u32, epp: u32) -> anyhow::Result<()> { self.record(format!("set_profile_epp {profile} {epp}")) }
}
```

- [ ] **Step 4: Run** `cargo test -p armouryd` — Expected: all pass (the zbus proxies compile; `RawCurve` = `(String, [u8;8], [u8;8], bool)` must serialize as `(s(yyyyyyyy)(yyyyyyyy)b)` — if zvariant encodes `[u8; 8]` as `ay`, change `RawCurve` to use 8-tuples `(u8, u8, u8, u8, u8, u8, u8, u8)` with `From` helpers in `fan.rs`, and ledger the ruling; the on-device check in Task 11 is the proof).
- [ ] **Step 5: Commit** `git add -A crates/armouryd && git commit -m "feat(armouryd): asusd D-Bus client for profiles, fan curves, armoury attrs, EPP"`

---

### Task 6: Performance readings from sysfs

**Files:** Modify `crates/armouryd/src/hw/sysfs.rs`, `crates/armouryd/src/state.rs`

**Interfaces — Produces:** `sysfs::HWMON_DIR = "sys/class/hwmon"`, `sysfs::PROFILE_CHOICES = "sys/firmware/acpi/platform_profile_choices"`, `sysfs::NO_TURBO = "sys/devices/system/cpu/intel_pstate/no_turbo"`, `fn find_hwmon(sys: &dyn Sysfs, name: &str) -> Option<String>` (returns `sys/class/hwmon/hwmonN`); `state::perf_state(sys: &dyn Sysfs) -> PerfState`; `collect` fills `Snapshot.perf`.

- [ ] **Step 1: Failing test** (in `state.rs` tests; extend `machine()` entries with the lines below and add the test)

```rust
            ("sys/firmware/acpi/platform_profile_choices", "quiet balanced performance"),
            ("sys/class/hwmon/hwmon8/name", "coretemp"),
            ("sys/class/hwmon/hwmon8/temp1_input", "72000"),
            ("sys/class/hwmon/hwmon9/name", "asus"),
            ("sys/class/hwmon/hwmon9/fan1_input", "3300"),
            ("sys/class/hwmon/hwmon9/fan2_input", "5200"),
            ("sys/class/power_supply/BAT0/power_now", "18250000"),
            ("sys/devices/system/cpu/intel_pstate/no_turbo", "0"),
```

```rust
    #[tokio::test]
    async fn perf_readings() {
        let s = collect(&machine(), &FakeGfx::default(), &FakeServices::default(), ControlMode::Observe).await;
        assert_eq!(s.perf.profile, Some(Profile::Performance));
        assert_eq!(s.perf.choices, vec![Profile::Quiet, Profile::Balanced, Profile::Performance]);
        assert_eq!(s.perf.cpu_temp_c, Some(72.0));
        assert_eq!((s.perf.cpu_fan_rpm, s.perf.gpu_fan_rpm), (Some(3300), Some(5200)));
        assert_eq!(s.perf.power_draw_w, Some(18.25));
        assert_eq!(s.perf.cpu_boost, Some(true));
    }
```
(add `Profile` to the test module's imports via `use armoury_proto::Profile;`)

- [ ] **Step 2: Run** `cargo test -p armouryd state` — Expected: FAIL (`perf` field default / assertion).

- [ ] **Step 3: Implement.** `sysfs.rs` add:

```rust
pub const HWMON_DIR: &str = "sys/class/hwmon";
pub const PROFILE_CHOICES: &str = "sys/firmware/acpi/platform_profile_choices";
pub const NO_TURBO: &str = "sys/devices/system/cpu/intel_pstate/no_turbo";

/// Path of the hwmon directory whose `name` is `name` (hwmon numbering is not stable).
pub fn find_hwmon(sys: &dyn Sysfs, name: &str) -> Option<String> {
    sys.list(HWMON_DIR).into_iter()
        .map(|h| format!("{HWMON_DIR}/{h}"))
        .find(|p| sys.read(&format!("{p}/name")).as_deref() == Some(name))
}
```

`state.rs`: in `collect` add `perf: perf_state(sys),`; add:

```rust
pub fn perf_state(sys: &dyn Sysfs) -> PerfState {
    let num = |p: String| sys.read(&p).and_then(|v| v.parse::<i64>().ok());
    let coretemp = sysfs::find_hwmon(sys, "coretemp");
    let asus = sysfs::find_hwmon(sys, "asus");
    let battery = sys.list(sysfs::POWER_SUPPLY_DIR).into_iter()
        .find(|n| sys.read(&format!("{}/{n}/type", sysfs::POWER_SUPPLY_DIR)).as_deref() == Some("Battery"));
    PerfState {
        profile: sys.read(sysfs::PLATFORM_PROFILE).as_deref().and_then(Profile::from_sysfs),
        choices: sys.read(sysfs::PROFILE_CHOICES).unwrap_or_default().split_whitespace().filter_map(Profile::from_sysfs).collect(),
        cpu_temp_c: coretemp.and_then(|h| num(format!("{h}/temp1_input"))).map(|m| m as f32 / 1000.0),
        cpu_fan_rpm: asus.as_ref().and_then(|h| num(format!("{h}/fan1_input"))).map(|v| v as u32),
        gpu_fan_rpm: asus.as_ref().and_then(|h| num(format!("{h}/fan2_input"))).map(|v| v as u32),
        power_draw_w: battery.and_then(|b| num(format!("{}/{b}/power_now", sysfs::POWER_SUPPLY_DIR))).map(|uw| uw as f32 / 1_000_000.0),
        cpu_boost: sys.read(sysfs::NO_TURBO).map(|v| v == "0"),
    }
}
```
and import `PerfState`, `Profile` from `armoury_proto`.

- [ ] **Step 4: Run** `cargo test -p armouryd state` — Expected: 5 passed.
- [ ] **Step 5: Commit** `git add -A crates/armouryd && git commit -m "feat(armouryd): CPU temp, fan RPM, power draw, boost and profile readings"`

---

### Task 7: apply_mode

**Files:** Create `crates/armouryd/src/features/perf.rs`; `features/mod.rs` add `pub mod perf;`

**Interfaces — Consumes:** `Asusd`, `Services`, `Limit`, `PKEXEC`, `ROOT_HELPER`. **Produces:** `pub async fn apply_mode(profile: Profile, s: &ModeSettings, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String>` (errors, empty = success). Order: if any limit is set → `set_ppt_group(true)` then each set limit via `armoury_set(limit.attr(), v)`; then `set_profile_epp(profile.to_asusd(), epp.to_asusd())` if set; then `pkexec armoury-root cpu-boost on|off` if set. Every step is attempted even if an earlier one fails. Each asusd call goes through `with_retry`.

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeAsusd, FakeServices};
    use armoury_proto::Epp;

    fn full() -> ModeSettings {
        ModeSettings { pl1: Some(120), pl2: Some(150), nv_boost: Some(25), nv_temp: Some(87), epp: Some(Epp::Performance), cpu_boost: Some(false) }
    }

    #[tokio::test]
    async fn applies_everything_in_order() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        assert!(apply_mode(Profile::Performance, &full(), &a, &s).await.is_empty());
        assert_eq!(*a.calls.lock().unwrap(), [
            "set_ppt_group true",
            "armoury_set ppt_pl1_spl 120",
            "armoury_set ppt_pl2_sppt 150",
            "armoury_set nv_dynamic_boost 25",
            "armoury_set nv_temp_target 87",
            "set_profile_epp 1 1",
        ]);
        assert_eq!(*s.calls.lock().unwrap(), [format!("{PKEXEC} {ROOT_HELPER} cpu-boost off")]);
    }

    #[tokio::test]
    async fn empty_settings_touch_nothing() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        assert!(apply_mode(Profile::Quiet, &ModeSettings::default(), &a, &s).await.is_empty());
        assert!(a.calls.lock().unwrap().is_empty() && s.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn apply_continues_after_error() {
        let (a, s) = (FakeAsusd::default(), FakeServices::default());
        *a.fail_on.lock().unwrap() = Some("ppt_pl1_spl".into());
        let errs = apply_mode(Profile::Performance, &full(), &a, &s).await;
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("PL1"), "{errs:?}");
        assert!(a.calls.lock().unwrap().iter().any(|c| c.contains("nv_temp_target")));
        assert_eq!(s.calls.lock().unwrap().len(), 1);
    }
}
```

- [ ] **Step 2: Run** `cargo test -p armouryd features::perf` — Expected: FAIL.

- [ ] **Step 3: Implement.** Make `Limit::label` `pub` in `limits.rs`, then:

```rust
use crate::control::{PKEXEC, ROOT_HELPER};
use crate::features::limits::Limit;
use crate::hw::{Asusd, Services, with_retry};
use armoury_proto::{ModeSettings, Profile};

/// Applies every setting that is Some; keeps going after failures and returns their messages.
pub async fn apply_mode(profile: Profile, s: &ModeSettings, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let mut errors = Vec::new();
    if Limit::ALL.iter().any(|l| l.value(s).is_some()) {
        // asusd only writes PPT values to hardware while its tuning group is enabled
        if let Err(e) = with_retry(|| asusd.set_ppt_group(true)).await { errors.push(format!("enable tuning: {e:#}")); }
        for l in Limit::ALL {
            if let Some(v) = l.value(s) {
                if let Err(e) = with_retry(|| asusd.armoury_set(l.attr(), v)).await { errors.push(format!("{}: {e:#}", l.label())); }
            }
        }
    }
    if let Some(epp) = s.epp {
        if let Err(e) = with_retry(|| asusd.set_profile_epp(profile.to_asusd(), epp.to_asusd())).await { errors.push(format!("EPP: {e:#}")); }
    }
    if let Some(on) = s.cpu_boost {
        let arg = if on { "on" } else { "off" };
        if let Err(e) = svc.run(&[PKEXEC, ROOT_HELPER, "cpu-boost", arg]).await { errors.push(format!("CPU boost: {e:#}")); }
    }
    errors
}
```

- [ ] **Step 4: Run** — Expected: 3 passed.
- [ ] **Step 5: Commit** `git add -A crates/armouryd && git commit -m "feat(armouryd): apply per-mode power limits, EPP and CPU boost"`

---

### Task 8: Daemon handlers, auto-apply on mode change, re-apply timer

**Files:** Modify `crates/armouryd/src/ipc.rs`, `crates/armouryd/src/main.rs`

**Interfaces — Consumes:** everything above. **Produces:** `Daemon::new(sys, gfx, svc, asusd: Box<dyn Asusd>, control, config_path: PathBuf) -> Arc<Daemon>` (loads config; a load error is kept in `config_error` and logged); `Daemon::tick(&self)` (called by `poll_loop` each poll: refresh, then if active: apply on profile change, and re-apply limits when `reapply_power_secs` elapsed). Handlers:
- `SetProfile` → require active, `asusd.set_profile`, then `self.tick()` (applies that mode), returns snapshot.
- `NextProfile` → require active, `asusd.next_profile`, `tick`.
- `FanCurves{profile}` → `asusd.fan_curves` → `Vec<FanCurve>` (works in observe mode only if asusd runs; otherwise error text from asusd).
- `SetFanCurve` → require active, `fan::validate`, `asusd.set_fan_curve(to_raw)`; returns updated curves.
- `ResetFanCurves` → require active, `asusd.reset_fan_curves`; returns curves.
- `ModeSettings{profile}` → stored settings plus bounds: `{"settings": ModeSettings, "bounds": {"pl1":[min,max],...}}`.
- `SetModeSettings` → require active, `limits::validate` with bounds from `asusd.armoury_range` (fallback on error), merge into stored (fields `Some` overwrite), if `profile` is the current one → `apply_mode`; on any apply error return `Response::err` joined with `"; "` and **do not save**; else save config and return stored settings.

Last-applied profile is tracked in `applied: tokio::sync::Mutex<Option<Profile>>`; the re-apply clock in `last_reapply: std::sync::Mutex<tokio::time::Instant>`.

- [ ] **Step 1: Failing tests** (in `ipc.rs` tests; update the `daemon()` helper and `takeover_request_reports_failure_as_error` to the new constructor; they take `Box::new(FakeAsusd::default())` and `dir.join("config.toml")`). Add a helper that builds an active daemon with shared fakes:

```rust
    struct Rig { d: Arc<Daemon>, sys: Arc<FakeSysfs>, asusd: Arc<FakeAsusd>, dir: tempfile::TempDir }

    fn rig(active: bool) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let sys = Arc::new(FakeSysfs::with(&[("sys/firmware/acpi/platform_profile", "balanced")]));
        let asusd = Arc::new(FakeAsusd::default());
        let mut ctl = Control::load(dir.path());
        if active { ctl.set(ControlMode::Active).unwrap(); }
        let d = Daemon::new(Box::new(sys.clone()), Box::new(FakeGfx::default()), Box::new(FakeServices::default()),
            Box::new(asusd.clone()), ctl, dir.path().join("config.toml"));
        Rig { d, sys, asusd, dir }
    }

    fn set_mode(profile: &str, pl1: i32, pl2: i32) -> Request {
        serde_json::from_value(serde_json::json!({"cmd":"set_mode_settings","profile":profile,"settings":{"pl1":pl1,"pl2":pl2}})).unwrap()
    }

    #[tokio::test]
    async fn observe_mode_never_applies() {
        let r = rig(false);
        let resp = r.d.handle(set_mode("balanced", 60, 80)).await;
        assert!(!resp.ok && resp.error.unwrap().contains("observe"));
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "performance".into());
        r.d.tick().await;
        assert!(r.asusd.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_mode_settings_applies_current_and_saves() {
        let r = rig(true);
        r.d.tick().await; // first tick records current profile (nothing stored yet)
        let resp = r.d.handle(set_mode("balanced", 60, 80)).await;
        assert!(resp.ok, "{resp:?}");
        assert!(r.asusd.calls.lock().unwrap().contains(&"armoury_set ppt_pl1_spl 60".to_string()));
        let text = std::fs::read_to_string(r.dir.path().join("config.toml")).unwrap();
        assert!(text.contains("pl1 = 60"), "{text}");
    }

    #[tokio::test]
    async fn set_mode_settings_for_other_mode_only_saves() {
        let r = rig(true);
        r.d.tick().await;
        assert!(r.d.handle(set_mode("performance", 120, 150)).await.ok);
        assert!(!r.asusd.calls.lock().unwrap().iter().any(|c| c.contains("ppt")));
    }

    #[tokio::test]
    async fn external_profile_change_applies_mode() {
        let r = rig(true);
        r.d.tick().await;
        assert!(r.d.handle(set_mode("performance", 120, 150)).await.ok);
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "performance".into()); // Fn+F5
        r.d.tick().await;
        assert!(r.asusd.calls.lock().unwrap().contains(&"armoury_set ppt_pl2_sppt 150".to_string()));
    }

    #[tokio::test]
    async fn invalid_settings_rejected_and_not_saved() {
        let r = rig(true);
        let resp = r.d.handle(set_mode("balanced", 140, 100)).await;
        assert!(resp.error.unwrap().contains("PL1 must not exceed PL2"));
        assert!(!r.dir.path().join("config.toml").exists());
        assert!(r.asusd.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_set_does_not_save() {
        let r = rig(true);
        r.d.tick().await;
        *r.asusd.fail_on.lock().unwrap() = Some("ppt_pl1_spl".into());
        let resp = r.d.handle(set_mode("balanced", 60, 80)).await;
        assert!(!resp.ok && resp.error.unwrap().contains("PL1"));
        assert!(!r.dir.path().join("config.toml").exists());
    }

    #[tokio::test]
    async fn fan_curve_validation_and_set() {
        let r = rig(true);
        let bad = serde_json::json!({"cmd":"set_fan_curve","profile":"quiet","curve":{"fan":"cpu","temps":[40,50,60,70,80,90,95,97],"percent":[50,40,50,60,70,80,90,100],"enabled":true}});
        let resp = r.d.handle(serde_json::from_value(bad).unwrap()).await;
        assert!(resp.error.unwrap().contains("fan speed"));
        let good = serde_json::json!({"cmd":"set_fan_curve","profile":"quiet","curve":{"fan":"cpu","temps":[40,50,60,70,80,90,95,97],"percent":[0,10,20,30,40,60,80,100],"enabled":true}});
        let resp = r.d.handle(serde_json::from_value(good).unwrap()).await;
        assert!(resp.ok, "{resp:?}");
        assert_eq!(resp.data.unwrap()[0]["percent"][7], 100);
        assert!(r.asusd.calls.lock().unwrap().contains(&"set_fan_curve 2 CPU".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn reapply_timer() {
        let r = rig(true);
        r.d.tick().await;
        assert!(r.d.handle(set_mode("balanced", 60, 80)).await.ok);
        r.asusd.calls.lock().unwrap().clear();
        r.d.config.lock().await.reapply_power_secs = 10;
        tokio::time::advance(Duration::from_secs(5)).await;
        r.d.tick().await;
        assert!(r.asusd.calls.lock().unwrap().is_empty());
        tokio::time::advance(Duration::from_secs(6)).await;
        r.d.tick().await;
        assert!(r.asusd.calls.lock().unwrap().contains(&"armoury_set ppt_pl1_spl 60".to_string()));
    }
```

`Sysfs`/`Asusd` for `Arc<T>`: add blanket impls in `hw/mod.rs` so the tests can keep a handle:

```rust
impl<T: Sysfs + ?Sized> Sysfs for std::sync::Arc<T> {
    fn read(&self, rel: &str) -> Option<String> { (**self).read(rel) }
    fn list(&self, rel: &str) -> Vec<String> { (**self).list(rel) }
}
```
and the same forwarding impl of `Asusd` for `Arc<T>` (each method forwards to `(**self)`).

- [ ] **Step 2: Run** `cargo test -p armouryd ipc` — Expected: FAIL (constructor arity, missing `tick`).

- [ ] **Step 3: Implement** in `ipc.rs`:

Struct fields added: `asusd: Box<dyn Asusd>`, `pub config: Mutex<Config>`, `config_path: PathBuf`, `config_error: Option<String>`, `applied: Mutex<Option<Profile>>`, `last_reapply: std::sync::Mutex<tokio::time::Instant>`.

```rust
    pub fn new(sys: Box<dyn Sysfs>, gfx: Box<dyn Gfx>, svc: Box<dyn Services>, asusd: Box<dyn Asusd>, control: Control, config_path: PathBuf) -> Arc<Self> {
        let (config, config_error) = Config::load(&config_path);
        if let Some(e) = &config_error { eprintln!("armouryd: config ignored: {e}"); }
        let mode = control.mode_handle();
        Arc::new(Self {
            sys, gfx, svc, asusd, control: Mutex::new(control), mode, snap: watch::channel(None).0,
            config: Mutex::new(config), config_path, config_error,
            applied: Mutex::new(None), last_reapply: std::sync::Mutex::new(tokio::time::Instant::now()),
        })
    }

    /// One poll: refresh, then (active only) apply the mode's settings when the
    /// profile changed from any source, and re-apply limits on the configured interval.
    pub async fn tick(&self) {
        let snap = self.refresh().await;
        if self.mode.get() != ControlMode::Active { return; }
        let Some(profile) = snap.perf.profile else { return };
        let mut applied = self.applied.lock().await;
        let settings = self.config.lock().await.mode(profile);
        let changed = *applied != Some(profile);
        let secs = self.config.lock().await.reapply_power_secs;
        let due = secs > 0 && self.last_reapply.lock().unwrap().elapsed() >= Duration::from_secs(secs as u64);
        if changed || due {
            let errs = apply_mode(profile, &settings, &*self.asusd, &*self.svc).await;
            for e in &errs { eprintln!("armouryd: apply {profile:?}: {e}"); }
            *applied = Some(profile);
            *self.last_reapply.lock().unwrap() = tokio::time::Instant::now();
        }
    }

    async fn write_guard(&self) -> Result<(), Response> {
        self.control.lock().await.require_active().map_err(|e| Response::err(e.to_string()))
    }

    async fn curves(&self, p: Profile) -> Response {
        match with_retry(|| self.asusd.fan_curves(p.to_asusd())).await {
            Ok(raw) => Response::ok(serde_json::to_value(raw.iter().filter_map(fan::from_raw).collect::<Vec<_>>()).unwrap()),
            Err(e) => Response::err(format!("fan curves: {e:#}")),
        }
    }

    async fn bounds(&self) -> impl Fn(Limit) -> Bounds {
        let mut found = Vec::new();
        for l in Limit::ALL {
            found.push((l, limits::bounds(l, with_retry(|| self.asusd.armoury_range(l.attr())).await.ok())));
        }
        move |l| found.iter().find(|(k, _)| *k == l).map(|(_, b)| *b).unwrap()
    }
```

`handle` gains arms (keep existing ones; remove the temporary `_` arm):

```rust
            Request::SetProfile { profile } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = with_retry(|| self.asusd.set_profile(profile.to_asusd())).await { return Response::err(format!("{e:#}")); }
                self.tick().await;
                Response::ok(serde_json::to_value(self.refresh().await).unwrap())
            }
            Request::NextProfile => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = with_retry(|| self.asusd.next_profile()).await { return Response::err(format!("{e:#}")); }
                self.tick().await;
                Response::ok(serde_json::to_value(self.refresh().await).unwrap())
            }
            Request::FanCurves { profile } => self.curves(profile).await,
            Request::SetFanCurve { profile, curve } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = fan::validate(&curve) { return Response::err(e); }
                if let Err(e) = with_retry(|| self.asusd.set_fan_curve(profile.to_asusd(), fan::to_raw(&curve))).await { return Response::err(format!("{e:#}")); }
                self.curves(profile).await
            }
            Request::ResetFanCurves { profile } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = with_retry(|| self.asusd.reset_fan_curves(profile.to_asusd())).await { return Response::err(format!("{e:#}")); }
                self.curves(profile).await
            }
            Request::ModeSettings { profile } => {
                let b = self.bounds().await;
                let bounds: serde_json::Map<String, serde_json::Value> = Limit::ALL.iter()
                    .map(|l| (l.key().to_string(), serde_json::json!([b(*l).min, b(*l).max]))).collect();
                Response::ok(serde_json::json!({"settings": self.config.lock().await.mode(profile), "bounds": bounds}))
            }
            Request::SetModeSettings { profile, settings } => {
                if let Err(r) = self.write_guard().await { return r; }
                let merged = merge(self.config.lock().await.mode(profile), settings);
                if let Err(e) = limits::validate(&merged, self.bounds().await) { return Response::err(e); }
                if self.refresh().await.perf.profile == Some(profile) {
                    let errs = apply_mode(profile, &settings, &*self.asusd, &*self.svc).await;
                    if !errs.is_empty() { return Response::err(errs.join("; ")); }
                }
                let mut cfg = self.config.lock().await;
                cfg.modes.insert(profile, merged);
                if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                Response::ok(serde_json::to_value(merged).unwrap())
            }
```

Helpers at module level:

```rust
/// Fields present in `new` overwrite `old`.
fn merge(old: ModeSettings, new: ModeSettings) -> ModeSettings {
    ModeSettings {
        pl1: new.pl1.or(old.pl1), pl2: new.pl2.or(old.pl2),
        nv_boost: new.nv_boost.or(old.nv_boost), nv_temp: new.nv_temp.or(old.nv_temp),
        epp: new.epp.or(old.epp), cpu_boost: new.cpu_boost.or(old.cpu_boost),
    }
}
```

Add `Limit::key(self) -> &'static str` (`pl1`, `pl2`, `nv_boost`, `nv_temp`) in `limits.rs`. `poll_loop` calls `self.tick().await` instead of `refresh`. Imports: `crate::config::Config`, `crate::features::{fan, limits::{self, Bounds, Limit}, perf::apply_mode}`, `crate::hw::{Asusd, with_retry}`, `armoury_proto::{ControlMode, ModeSettings, Profile}`, `std::path::PathBuf`.

The `reapply_timer` test sets `config.reapply_power_secs` directly; editing it from the UI/CLI comes with the settings page (Plan 5).

`main.rs`: build `AsusdClient::new().await?` and pass `Box::new(asusd)` plus `config_path(&home)`; compute `home` once and use it for both `state_dir` and `config_path`.

- [ ] **Step 4: Run** `cargo test --workspace` — Expected: all pass.
- [ ] **Step 5: Commit** `git add -A crates && git commit -m "feat(armouryd): profile, fan-curve and mode-settings requests with auto-apply and re-apply"`

---

### Task 9: armoury-root cpu-boost

**Files:** Modify `crates/armoury-root/src/lib.rs`, `crates/armoury-root/src/main.rs`

**Interfaces — Produces:** `pub const NO_TURBO: &str = "/sys/devices/system/cpu/intel_pstate/no_turbo"`; `pub fn no_turbo_value(on: &str) -> Result<&'static str, String>` (`"on"`→`"0"`, `"off"`→`"1"`, else error); subcommand `cpu-boost <on|off>`.

- [ ] **Step 1: Failing test** (lib tests)

```rust
    #[test]
    fn cpu_boost_arg() {
        assert_eq!(no_turbo_value("on"), Ok("0"));
        assert_eq!(no_turbo_value("off"), Ok("1"));
        assert!(no_turbo_value("1; rm -rf /").is_err());
    }
```

- [ ] **Step 2: Run** `cargo test -p armoury-root` — Expected: FAIL.

- [ ] **Step 3: Implement.** lib:

```rust
pub const NO_TURBO: &str = "/sys/devices/system/cpu/intel_pstate/no_turbo";

pub fn no_turbo_value(on: &str) -> Result<&'static str, String> {
    match on { "on" => Ok("0"), "off" => Ok("1"), other => Err(format!("cpu-boost takes on|off, got {other:?}")) }
}
```

main: add `/// Turn CPU turbo boost on or off\n CpuBoost { state: String },` to `Cmd`, and the arm:

```rust
        Cmd::CpuBoost { state } => no_turbo_value(&state).map_err(anyhow::Error::msg)
            .and_then(|v| std::fs::write(NO_TURBO, v).context("write no_turbo")),
```

- [ ] **Step 4: Run** — Expected: 15 passed.
- [ ] **Step 5: Commit** `git add -A crates/armoury-root && git commit -m "feat(armoury-root): cpu-boost on|off"`

---

### Task 10: CLI subcommands

**Files:** Modify `crates/armoury/src/main.rs`

**Interfaces — Produces:**
- `armoury profile` (show), `armoury profile set <quiet|balanced|performance>`, `armoury profile next`
- `armoury fan <profile>` (show), `armoury fan <profile> set <cpu|gpu> <T:P,T:P,…8 points>`, `armoury fan <profile> reset`
- `armoury mode <profile>` (show settings + bounds), `armoury mode <profile> set [--pl1 N] [--pl2 N] [--nv-boost N] [--nv-temp N] [--epp E] [--cpu-boost on|off]`
- `fn parse_curve(fan: Fan, s: &str) -> Result<FanCurve, String>` (exactly 8 `T:P` pairs, `%` optional).
- `status` summary gains `Mode       performance · CPU 72°C · fans 3300/5200 rpm · 18.2 W`.

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn parse_curve_points() {
        let c = parse_curve(armoury_proto::Fan::Gpu, "40:0,50:10%,60:20,70:30,80:40,90:60,95:80,97:100").unwrap();
        assert_eq!(c.temps, [40, 50, 60, 70, 80, 90, 95, 97]);
        assert_eq!(c.percent[1], 10);
        assert!(c.enabled);
        assert!(parse_curve(armoury_proto::Fan::Cpu, "40:0,50:10").unwrap_err().contains("8"));
        assert!(parse_curve(armoury_proto::Fan::Cpu, "a:b,1:1,1:1,1:1,1:1,1:1,1:1,1:1").is_err());
    }

    #[test]
    fn summary_has_perf_line() {
        let mut s = Snapshot::default();
        s.perf.profile = Some(armoury_proto::Profile::Performance);
        s.perf.cpu_temp_c = Some(72.0);
        s.perf.cpu_fan_rpm = Some(3300);
        s.perf.gpu_fan_rpm = Some(5200);
        s.perf.power_draw_w = Some(18.25);
        assert!(summary(&s).contains("Mode       performance · CPU 72°C · fans 3300/5200 rpm · 18.2 W"), "{}", summary(&s));
    }
```

- [ ] **Step 2: Run** `cargo test -p armoury` — Expected: FAIL.

- [ ] **Step 3: Implement.** Add to `Cmd`:

```rust
    /// Show or change the performance mode
    Profile {
        #[command(subcommand)]
        action: Option<ProfileAction>,
    },
    /// Show or edit a mode's fan curves
    Fan {
        profile: Profile,
        #[command(subcommand)]
        action: Option<FanAction>,
    },
    /// Show or edit a mode's power limits, EPP and CPU boost
    Mode {
        profile: Profile,
        #[command(subcommand)]
        action: Option<ModeAction>,
    },
```

```rust
#[derive(Subcommand)]
enum ProfileAction { Set { profile: Profile }, Next }

#[derive(Subcommand)]
enum FanAction {
    /// 8 points as TEMP:PERCENT, e.g. 40:0,50:10,60:20,70:30,80:40,90:60,95:80,97:100
    Set { fan: Fan, points: String },
    Reset,
}

#[derive(Subcommand)]
enum ModeAction {
    Set {
        #[arg(long)] pl1: Option<i32>,
        #[arg(long)] pl2: Option<i32>,
        #[arg(long)] nv_boost: Option<i32>,
        #[arg(long)] nv_temp: Option<i32>,
        #[arg(long)] epp: Option<Epp>,
        #[arg(long, value_parser = ["on", "off"])] cpu_boost: Option<String>,
    },
}
```

`Profile`, `Fan`, `Epp` need `clap::ValueEnum`: implement it in the CLI with small wrapper parsers instead of touching proto — use `#[arg(value_parser = parse_profile)]` style functions:

```rust
fn parse_profile(s: &str) -> Result<Profile, String> { Profile::from_sysfs(s).ok_or_else(|| format!("unknown mode {s} (quiet|balanced|performance)")) }
fn parse_fan(s: &str) -> Result<Fan, String> { serde_json::from_value(serde_json::json!(s)).map_err(|_| format!("unknown fan {s} (cpu|gpu|mid)")) }
fn parse_epp(s: &str) -> Result<Epp, String> { serde_json::from_value(serde_json::json!(s)).map_err(|_| format!("unknown EPP {s}")) }
```
and annotate each `profile: Profile` / `fan: Fan` / `epp: Option<Epp>` field with `#[arg(value_parser = parse_profile)]` etc.

```rust
fn parse_curve(fan: Fan, s: &str) -> Result<FanCurve, String> {
    let pts: Vec<&str> = s.split(',').map(str::trim).collect();
    if pts.len() != 8 { return Err(format!("need exactly 8 TEMP:PERCENT points, got {}", pts.len())); }
    let mut temps = [0u8; 8];
    let mut percent = [0u8; 8];
    for (i, p) in pts.iter().enumerate() {
        let (t, v) = p.split_once(':').ok_or_else(|| format!("point {p:?} is not TEMP:PERCENT"))?;
        temps[i] = t.trim_end_matches('c').parse().map_err(|_| format!("bad temperature in {p:?}"))?;
        percent[i] = v.trim_end_matches('%').parse().map_err(|_| format!("bad percent in {p:?}"))?;
    }
    Ok(FanCurve { fan, temps, percent, enabled: true })
}
```

`run` arms:

```rust
        Cmd::Profile { action } => {
            let req = match action {
                None => Request::Status,
                Some(ProfileAction::Set { profile }) => Request::SetProfile { profile },
                Some(ProfileAction::Next) => Request::NextProfile,
            };
            let s: Snapshot = serde_json::from_value(call(&req)?)?;
            println!("{}", s.perf.profile.map(|p| p.sysfs()).unwrap_or("-"));
        }
        Cmd::Fan { profile, action } => {
            let req = match action {
                None => Request::FanCurves { profile },
                Some(FanAction::Set { fan, points }) => Request::SetFanCurve { profile, curve: parse_curve(fan, &points).map_err(anyhow::Error::msg)? },
                Some(FanAction::Reset) => Request::ResetFanCurves { profile },
            };
            let curves: Vec<FanCurve> = serde_json::from_value(call(&req)?)?;
            for c in curves {
                let pts: Vec<String> = c.temps.iter().zip(c.percent).map(|(t, p)| format!("{t}:{p}")).collect();
                println!("{:?} {} {}", c.fan, if c.enabled { "on " } else { "off" }, pts.join(","));
            }
        }
        Cmd::Mode { profile, action } => {
            let req = match action {
                None => Request::ModeSettings { profile },
                Some(ModeAction::Set { pl1, pl2, nv_boost, nv_temp, epp, cpu_boost }) => Request::SetModeSettings {
                    profile,
                    settings: ModeSettings { pl1, pl2, nv_boost, nv_temp, epp, cpu_boost: cpu_boost.map(|v| v == "on") },
                },
            };
            println!("{}", serde_json::to_string_pretty(&call(&req)?)?);
        }
```

Summary line (insert after `Profile` line, keep `Profile` line):

```rust
    let perf = {
        let p = &s.perf;
        let mut parts = vec![p.profile.map(|m| m.sysfs().to_string()).unwrap_or_else(|| "-".into())];
        if let Some(t) = p.cpu_temp_c { parts.push(format!("CPU {t:.0}°C")); }
        if let (Some(c), Some(g)) = (p.cpu_fan_rpm, p.gpu_fan_rpm) { parts.push(format!("fans {c}/{g} rpm")); }
        if let Some(w) = p.power_draw_w { parts.push(format!("{w:.1} W")); }
        parts.join(" · ")
    };
```
and add `Mode       {perf}\n` to the format string.

- [ ] **Step 4: Run** `cargo test -p armoury` — Expected: 4 passed.
- [ ] **Step 5: Commit** `git add -A crates/armoury && git commit -m "feat(armoury): profile, fan and mode subcommands"`

---

### Task 11: Install and on-device verification

**Files:** none new. Rebuild and reinstall binaries (root helper changed).

- [ ] **Step 1: Full suite** `cargo test --workspace && tests/uninstall_test.sh` — Expected: all pass.

- [ ] **Step 2: Reinstall** (one pkexec prompt for the root helper):

```bash
cargo build --release --workspace
install -Dm755 target/release/armouryd ~/.local/bin/armouryd
install -Dm755 target/release/armoury ~/.local/bin/armoury
pkexec install -Dm755 "$PWD/target/release/armoury-root" /usr/local/lib/omarchy-armoury/armoury-root
systemctl --user restart armouryd
armoury status
```
Expected: status now has a `Mode` line (`performance · CPU …°C · fans …/… rpm · … W`), `Control observe`.

- [ ] **Step 3: On-device (user watching; G-Helper down ~2 min)**

```bash
armoury takeover
armoury fan performance                       # reads asusd curves
armoury mode balanced set --pl1 45 --pl2 65 --epp balance_power --cpu-boost on
armoury profile set balanced                  # applies stored balanced settings
cat /sys/firmware/acpi/platform_profile       # balanced
cat /sys/devices/system/cpu/intel_pstate/no_turbo   # 0
armoury profile set performance
armoury handback
armoury status
```
Expected: fan curves print as `Cpu on 51:13,55:29,…` (proves the `(s(yyyyyyyy)(yyyyyyyy)b)` encoding); every command succeeds; profile file follows; after handback `Control observe`, G-Helper running, asusd masked. G-Helper re-applies its own settings when it restarts.
