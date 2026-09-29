# Plan 5b — Manual Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fourth mode, Manual, holding named saved profiles (base mode, fan curves, CPU/GPU tuning). Silent, Balanced and Turbo become stock firmware modes. AC/battery switching can pick Manual.

**Architecture:**
- armouryd's apply loop moves from "settings per firmware profile" to a `Target` that is either `Stock(profile)` or `Manual { name, base }`.
- A new `features/manual.rs` owns the hardware sequences: stock = curves off, re-assert the mode, limits reset; manual = set base, write and enable curves, then the existing `apply_mode`.
- Profiles live in `config.toml` `[manual]`. The protocol swaps the per-mode fan and settings requests for manual-profile requests.
- The plugin merges the Fans and CPU/GPU pages into one Manual page.

**Tech Stack:** Rust (tokio, serde, toml, zbus via existing traits), QML (Quickshell 0.3.1, Omarchy `qs.Ui`).

**Spec:** `docs/superpowers/specs/2026-09-30-manual-mode-design.md` (extends `2026-09-29-omarchy-armoury-design.md`)

**Branch:** continue on `plan-5-ui` (it holds the UI this plan modifies); merge both together at the end.

## Global Constraints

- Mode ids on the wire: `quiet | balanced | performance | manual` (lowercase). UI labels: Silent / Balanced / Turbo / Manual.
- Hardware writes only in active mode. In observe mode every write request returns the existing "observe" error.
- G533ZW power limits: re-assert the mode first, enable the mode's fan curves before PPT (existing `apply_mode` does both). Never bypass `apply_mode` for limits.
- Never call `refresh()` / `snapshot()` while holding `self.config`: `snapshot()` takes that lock.
- A new profile's `base` defaults to `performance`.
- The last manual profile cannot be deleted.
- PL1 ≤ PL2 is enforced by the daemon, not only the UI.
- Every hardware test session ends with `armoury handback` (G-Helper running, asusd masked).

## Review Focus

1. **Takeover while `manual.enabled`:** the first active tick must re-enter Manual (set base, write curves). It must not treat the takeover as "the mode changed elsewhere". Test in Task 3.
2. **Our own base switch vs an outside change:** entering Manual on a different base than the current firmware profile must not immediately drop out of Manual on the next tick. Test in Task 3.
3. **Unsaved curve drags survive a settings change:** changing a slider or the base on the Manual page must not rebuild the fan graphs and lose unsaved drags. The graph model is set only on profile select. Probe in Task 5.
4. **Old config with a `[modes]` table:** must load (ignored, logged), not become `config.toml.bad`. Test in Task 1.
5. **Power-source flip right after start/takeover:** the first tick must not switch modes; only a real AC↔battery flip does. Test in Task 4.

---

## File Structure

**Proto**
- `crates/proto/src/lib.rs`: `ModeChoice`, `ManualProfile`, `ManualView`; `PerfState.mode` and `.manual_profile`; request set changes; `FanCurve.enabled` gets a serde default.

**armouryd**
- `crates/armouryd/src/config.rs`: `ManualConfig` and its profile rules; legacy `modes` ignored; `SystemConfig.profile_ac` and `profile_battery` become `Option<ModeChoice>`.
- `crates/armouryd/src/features/limits.rs`: `validate` rejects PL1 > PL2.
- `crates/armouryd/src/features/manual.rs` (new): `apply_stock`, `apply_manual`, `default_profile`, `fallback_curves`, `validate_profile`.
- `crates/armouryd/src/features/mod.rs`: `pub mod manual;`
- `crates/armouryd/src/hw/mod.rs`, `hw/asusd.rs`, `hw/fake.rs`:
  - `set_source_profiles` becomes `disable_source_switching`
  - `FakeAsusd` can mirror `set_profile` into a `FakeSysfs`
- `crates/armouryd/src/ipc.rs`:
  - `Target`-based apply loop
  - `choose_mode`, `ensure_manual_profile`, `reapply_manual`, `manual_view`, `follow_source_mode`
  - request handlers, snapshot fields, hotkey OSD, test rewrites

**CLI**
- `crates/armoury/src/main.rs`:
  - `profile set` takes a mode choice
  - `fan` and `mode` subcommands replaced by `manual [list|activate NAME]`
  - `auto profile` takes mode choices

**Plugin**
- `plugin/ManualPage.qml` (new).
- Deleted: `plugin/FansPage.qml`, `plugin/CpuGpuPage.qml`.
- `plugin/Window.qml`: Manual tile and page.
- `plugin/Widget.qml`: fourth mode button and icon.
- `plugin/SystemPage.qml`: Manual in the power-source rows.

---

### Task 1: Proto types and config model

**Files:**
- Modify: `crates/proto/src/lib.rs`, `crates/armouryd/src/config.rs`, `crates/armouryd/src/features/limits.rs`

**Interfaces:**
- Produces:
  - `armoury_proto::ModeChoice { Quiet, Balanced, Performance, Manual }` with:
    - `CYCLE: [ModeChoice; 4]`
    - `fn stock(self) -> Option<Profile>`
    - `fn label(self) -> &'static str`
    - `impl From<Profile>`
  - `armoury_proto::ManualProfile { name: String, base: Profile, curves: Vec<FanCurve>, settings: ModeSettings }`
  - `armoury_proto::ManualView { enabled: bool, active: Option<String>, profiles: Vec<ManualProfile> }`
  - `PerfState.mode: Option<ModeChoice>` and `PerfState.manual_profile: Option<String>`
  - `config::ManualConfig { enabled, active, profiles }` with:
    - `active_profile(&self) -> Option<&ManualProfile>`
    - `save(&mut self, p: ManualProfile, original: Option<&str>) -> Result<(), String>`
    - `delete(&mut self, name: &str) -> Result<(), String>`
    - `activate(&mut self, name: &str) -> Result<(), String>`
  - `Config.manual: ManualConfig`
  - `SystemConfig.profile_ac` and `profile_battery: Option<ModeChoice>`

- [ ] **Step 1: Write failing proto tests.** Append to the `tests` module in `crates/proto/src/lib.rs`:

```rust
    #[test]
    fn mode_choice_wire_names_and_cycle() {
        assert_eq!(serde_json::to_string(&ModeChoice::Manual).unwrap(), "\"manual\"");
        assert_eq!(serde_json::from_str::<ModeChoice>("\"performance\"").unwrap(), ModeChoice::Performance);
        assert_eq!(ModeChoice::CYCLE, [ModeChoice::Quiet, ModeChoice::Balanced, ModeChoice::Performance, ModeChoice::Manual]);
        assert_eq!(ModeChoice::Manual.stock(), None);
        assert_eq!(ModeChoice::Quiet.stock(), Some(Profile::Quiet));
        assert_eq!(ModeChoice::from(Profile::Performance).label(), "Turbo");
    }

    #[test]
    fn manual_profile_defaults() {
        let p: ManualProfile = serde_json::from_str(r#"{"name":"A"}"#).unwrap();
        assert_eq!((p.base, p.curves.len(), p.settings), (Profile::Performance, 0, ModeSettings::default()));
        // curves in a profile need no `enabled` flag
        let c: FanCurve = serde_json::from_str(r#"{"fan":"cpu","temps":[1,2,3,4,5,6,7,8],"percent":[0,0,0,0,0,0,0,0]}"#).unwrap();
        assert!(!c.enabled);
    }
```

- [ ] **Step 2: Run and confirm they fail.** `cargo test -p armoury-proto mode_choice manual_profile_defaults`. Expected: compile errors, since `ModeChoice` and `ManualProfile` don't exist yet.

- [ ] **Step 3: Implement the proto types.** In `crates/proto/src/lib.rs`, after `impl Profile`:

```rust
/// A mode the user picks: the three firmware modes or Manual (a saved profile on a base mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeChoice { Quiet, Balanced, Performance, Manual }

impl ModeChoice {
    /// Order of the mode key (Fn+F5) cycle.
    pub const CYCLE: [ModeChoice; 4] = [Self::Quiet, Self::Balanced, Self::Performance, Self::Manual];
    pub fn stock(self) -> Option<Profile> {
        match self { Self::Quiet => Some(Profile::Quiet), Self::Balanced => Some(Profile::Balanced), Self::Performance => Some(Profile::Performance), Self::Manual => None }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Quiet => "Silent", Self::Balanced => "Balanced", Self::Performance => "Turbo", Self::Manual => "Manual" }
    }
}

impl From<Profile> for ModeChoice {
    fn from(p: Profile) -> Self {
        match p { Profile::Quiet => Self::Quiet, Profile::Balanced => Self::Balanced, Profile::Performance => Self::Performance }
    }
}
```

In `FanCurve`, change the field to `#[serde(default)] pub enabled: bool,`. After `ModeSettings`, add:

```rust
fn default_base() -> Profile { Profile::Performance }

/// A saved Manual-mode profile: which firmware mode it runs on, its fan curves and its tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualProfile {
    pub name: String,
    #[serde(default = "default_base")]
    pub base: Profile,
    #[serde(default)]
    pub curves: Vec<FanCurve>,
    #[serde(default)]
    pub settings: ModeSettings,
}

/// What the UI sees of Manual mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ManualView {
    pub enabled: bool,
    pub active: Option<String>,
    pub profiles: Vec<ManualProfile>,
}
```

In `PerfState`, add after `profile`:

```rust
    /// The mode the user is in (Manual only while armouryd is active).
    #[serde(default)]
    pub mode: Option<ModeChoice>,
    /// Active manual profile name (even while a stock mode is on).
    #[serde(default)]
    pub manual_profile: Option<String>,
```

- [ ] **Step 4: Run proto tests.** `cargo test -p armoury-proto`. Expected: PASS.

- [ ] **Step 5: Write failing config tests.** Append to `crates/armouryd/src/config.rs` `tests`:

```rust
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
```

And in `crates/armouryd/src/features/limits.rs` `tests`:

```rust
    #[test]
    fn pl1_above_pl2_rejected() {
        let s = ModeSettings { pl1: Some(90), pl2: Some(80), ..Default::default() };
        assert!(validate(&s, |_| Bounds { min: 5, max: 150 }).unwrap_err().contains("PL1"));
    }
```

- [ ] **Step 6: Run and confirm they fail.** `cargo test -p armouryd config:: limits::`. Expected: compile errors (`ManualConfig`, `Config.manual`).

- [ ] **Step 7: Implement the config and the limits check.** In `config.rs`, replace the `modes` field and `mode()`:

```rust
    /// Pre-Manual per-mode settings: accepted on load and dropped (tuning lives in manual profiles).
    #[serde(skip_serializing)]
    pub modes: Option<toml::Value>,
    pub manual: ManualConfig,
```

Delete `pub fn mode(...)`. Remove the now-unused `BTreeMap`, `ModeSettings` and `Profile` imports if the compiler flags them. In `load`, change `Ok(c) => (c, None),` to:

```rust
                Ok(mut c) => {
                    if c.modes.take().is_some() { eprintln!("armouryd: ignoring the old [modes] table; tuning now lives in manual profiles"); }
                    (c, None)
                }
```

(`c` needs to be `Config` typed: `match toml::from_str::<Config>(&text)`.) Add:

```rust
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
```

In `SystemConfig`, change the two fields to `Option<armoury_proto::ModeChoice>` and update their doc comment: "Mode armouryd switches to on AC / on battery".

In `limits::validate`, before the final `Ok(())`:

```rust
    if let (Some(a), Some(b)) = (s.pl1, s.pl2) {
        if a > b { return Err(format!("PL1 ({a} W) can't be above PL2 ({b} W)")); }
    }
```

- [ ] **Step 8: Run the tests.** `cargo test -p armouryd config:: limits::`. Expected: the new tests PASS.

  The crate will **not** fully compile yet: `ipc.rs` still uses `cfg.mode()`, `modes` and `SetSourceProfile` with `Profile`. Run `cargo check -p armouryd 2>&1 | grep -c ^error` and note the count; Task 3 clears them. To keep the tree building between tasks, instead put a temporary shim in `impl Config`:

```rust
    /// Stock settings for every mode until the manual-mode apply loop lands (Task 3 removes this).
    pub fn mode(&self, _p: Profile) -> ModeSettings { ModeSettings::default() }
```

  In `ipc.rs`, replace `cfg.modes.insert(profile, merged);` with `let _ = merged;`, and the uv-probe condition's `cfg.modes.values().any(|m| m.uv_mv.is_some())` with `cfg.manual.profiles.iter().any(|p| p.settings.uv_mv.is_some())`. In `SetSourceProfile`, map `ac.and_then(ModeChoice::stock)` (and the same for battery) into the asusd call. Change the request type in proto to `SetSourceProfile { ac: Option<ModeChoice>, battery: Option<ModeChoice> }`. In the CLI, change `AutoAction::Profile` to `Option<ModeChoice>` with a `parse_mode_choice` value parser:

```rust
fn parse_mode_choice(s: &str) -> Result<ModeChoice, String> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| "expected quiet, balanced, performance or manual".to_string())
}
```

  Then `cargo test --workspace`. Expected: everything compiles. Tests that asserted per-mode limits being applied (`set_mode_settings_applies_current_and_saves`, `external_profile_change_applies_mode` and similar) now fail. Mark them `#[ignore = "rewritten in Task 3"]`; Task 3 rewrites or deletes each one.

- [ ] **Step 9: Commit.**

```bash
git add crates && git commit -m "feat(proto,config): ModeChoice, manual profiles in config, PL1 ≤ PL2"
```

---

### Task 2: Manual/stock hardware sequences

**Files:**
- Create: `crates/armouryd/src/features/manual.rs`
- Modify: `crates/armouryd/src/features/mod.rs`, `crates/armouryd/src/hw/fake.rs`

**Interfaces:**
- Consumes: `apply_mode`, `HwCtx` (`features/perf.rs`); `fan::{to_raw, from_raw, validate, RawCurve}`; `limits::{validate, Bounds, Limit}`.
- Produces:
  - `manual::apply_stock(p: Profile, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String>`
  - `manual::apply_manual(p: &ManualProfile, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String>`
  - `manual::default_profile(name: &str, raw: &[RawCurve]) -> ManualProfile`
  - `manual::fallback_curves() -> Vec<FanCurve>`
  - `manual::validate_profile(p: &ManualProfile, b: impl Fn(Limit) -> Bounds) -> Result<(), String>`
  - `FakeAsusd.mirror: Mutex<Option<Arc<FakeSysfs>>>`: when set, `set_profile` writes `sys/firmware/acpi/platform_profile`.

- [ ] **Step 1: Make the fake follow `set_profile`.** In `hw/fake.rs`, add a field to `FakeAsusd` (it derives `Default`, so `None` by default): `pub mirror: Mutex<Option<Arc<FakeSysfs>>>,`. Change `set_profile`:

```rust
    async fn set_profile(&self, p: u32) -> anyhow::Result<()> {
        self.record(format!("set_profile {p}"))?;
        if let (Some(sys), Some(prof)) = (self.mirror.lock().unwrap().as_ref(), armoury_proto::Profile::from_asusd(p)) {
            sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), prof.sysfs().into());
        }
        Ok(())
    }
```

(Add `use std::sync::Arc;` if missing.)

- [ ] **Step 2: Write failing tests.** Create `crates/armouryd/src/features/manual.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::fake::{FakeAsusd, FakeServices};
    use armoury_proto::{Fan, FanCurve, ModeSettings};

    fn curve(fan: Fan) -> FanCurve {
        FanCurve { fan, temps: [30, 40, 50, 60, 70, 80, 90, 100], percent: [0, 10, 20, 35, 55, 75, 90, 100], enabled: false }
    }

    #[tokio::test]
    async fn manual_sets_base_then_curves_then_limits() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        let p = ManualProfile { name: "G".into(), base: Profile::Performance, curves: vec![curve(Fan::Cpu), curve(Fan::Gpu)],
            settings: ModeSettings { pl1: Some(20), pl2: Some(40), ..Default::default() } };
        let errs = apply_manual(&p, HwCtx::default(), &asusd, &svc).await;
        assert!(errs.is_empty(), "{errs:?}");
        let calls = asusd.calls.lock().unwrap().clone();
        assert_eq!(&calls[..4], ["set_profile 1", "set_fan_curve 1 CPU", "set_fan_curve 1 GPU", "set_fan_curves_enabled 1 true"], "{calls:?}");
        // curves are written enabled even though the profile stores enabled=false
        assert!(asusd.curves.lock().unwrap()[&1].iter().all(|c| c.3));
        assert!(svc.calls.lock().unwrap().iter().any(|c| c.ends_with("set-limits pl1=20 pl2=40 cpu_boost=on")));
    }

    #[tokio::test]
    async fn stock_turns_curves_off_and_reasserts_mode() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        let errs = apply_stock(Profile::Quiet, HwCtx::default(), &asusd, &svc).await;
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(*asusd.calls.lock().unwrap(), ["set_fan_curves_enabled 2 false", "set_profile 2"]);
        assert!(svc.calls.lock().unwrap().iter().any(|c| c.ends_with("set-limits cpu_boost=on")));
    }

    #[tokio::test]
    async fn manual_stops_when_base_cannot_be_set() {
        let (asusd, svc) = (FakeAsusd::default(), FakeServices::default());
        *asusd.fail_on.lock().unwrap() = Some("set_profile".into());
        let p = ManualProfile { name: "G".into(), base: Profile::Quiet, curves: vec![curve(Fan::Cpu)], settings: ModeSettings::default() };
        let errs = apply_manual(&p, HwCtx::default(), &asusd, &svc).await;
        assert!(errs[0].contains("base mode"), "{errs:?}");
        assert!(!asusd.calls.lock().unwrap().iter().any(|c| c.starts_with("set_fan_curve")), "no curves on the wrong mode");
    }

    #[test]
    fn default_profile_uses_asusd_curves_or_fallback() {
        let raw: Vec<crate::features::fan::RawCurve> = vec![("CPU".into(), [0, 25, 51, 76, 102, 153, 204, 255], [30, 40, 50, 60, 70, 80, 90, 100], true)];
        let p = default_profile("Manual 1", &raw);
        assert_eq!((p.name.as_str(), p.base, p.curves.len()), ("Manual 1", Profile::Performance, 1));
        assert_eq!(default_profile("X", &[]).curves, fallback_curves());
        assert_eq!(fallback_curves().iter().map(|c| c.fan).collect::<Vec<_>>(), [Fan::Cpu, Fan::Gpu]);
    }

    #[test]
    fn profile_validation() {
        let b = |_| crate::features::limits::Bounds { min: 5, max: 150 };
        let mut p = default_profile("A", &[]);
        assert!(validate_profile(&p, b).is_ok());
        p.curves[0].percent[3] = 0; // decreasing
        assert!(validate_profile(&p, b).is_err());
        let mut q = default_profile("A", &[]);
        q.curves.push(q.curves[0].clone());
        assert!(validate_profile(&q, b).unwrap_err().contains("twice"));
    }
}
```

Add `pub mod manual;` to `features/mod.rs`.

- [ ] **Step 3: Run and confirm they fail.** `cargo test -p armouryd manual::`. Expected: compile errors (functions missing).

- [ ] **Step 4: Implement.** At the top of `manual.rs`:

```rust
//! Manual mode vs the stock firmware modes.
//! Stock: the mode's custom fan curves off, the mode re-asserted (the firmware reloads
//! its own power limits on a thermal-policy change), CPU boost on, undervolt / GPU
//! clocks back to stock. Manual: base mode, the profile's curves written into that
//! mode's asusd slot and enabled, then the usual apply_mode for limits and GPU.
use crate::features::fan::{self, RawCurve};
use crate::features::limits::{self, Bounds, Limit};
use crate::features::perf::{apply_mode, HwCtx};
use crate::hw::{with_retry, Asusd, Services};
use armoury_proto::{Fan, FanCurve, ManualProfile, ModeSettings, Profile};

pub async fn apply_stock(p: Profile, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(e) = with_retry(|| asusd.set_fan_curves_enabled(p.to_asusd(), false)).await {
        errors.push(format!("fan curves off: {e:#}"));
    }
    if let Err(e) = with_retry(|| asusd.set_profile(p.to_asusd())).await {
        errors.push(format!("re-assert mode: {e:#}"));
    }
    errors.extend(apply_mode(p, &ModeSettings::default(), ctx, asusd, svc).await);
    errors
}

pub async fn apply_manual(p: &ManualProfile, ctx: HwCtx, asusd: &dyn Asusd, svc: &dyn Services) -> Vec<String> {
    let base = p.base.to_asusd();
    if let Err(e) = with_retry(|| asusd.set_profile(base)).await {
        return vec![format!("set base mode: {e:#}")];
    }
    let mut errors = Vec::new();
    for c in &p.curves {
        let raw = fan::to_raw(&FanCurve { enabled: true, ..c.clone() });
        if let Err(e) = with_retry(|| asusd.set_fan_curve(base, raw.clone())).await {
            errors.push(format!("{} fan curve: {e:#}", c.fan.asusd_name()));
        }
    }
    if let Err(e) = with_retry(|| asusd.set_fan_curves_enabled(base, true)).await {
        errors.push(format!("enable fan curves: {e:#}"));
    }
    errors.extend(apply_mode(p.base, &p.settings, ctx, asusd, svc).await);
    errors
}

/// Used when asusd can't report curves.
pub fn fallback_curves() -> Vec<FanCurve> {
    [Fan::Cpu, Fan::Gpu].into_iter()
        .map(|fan| FanCurve { fan, temps: [30, 40, 50, 60, 70, 80, 90, 100], percent: [0, 10, 20, 35, 55, 75, 90, 100], enabled: true })
        .collect()
}

/// A new profile on Turbo with the given asusd curves (or the fallback) and no tuning.
pub fn default_profile(name: &str, raw: &[RawCurve]) -> ManualProfile {
    let curves: Vec<FanCurve> = raw.iter().filter_map(fan::from_raw).collect();
    ManualProfile {
        name: name.to_string(),
        base: Profile::Performance,
        curves: if curves.is_empty() { fallback_curves() } else { curves },
        settings: ModeSettings::default(),
    }
}

pub fn validate_profile(p: &ManualProfile, b: impl Fn(Limit) -> Bounds) -> Result<(), String> {
    for (i, c) in p.curves.iter().enumerate() {
        fan::validate(c).map_err(|e| format!("{} fan: {e}", c.fan.asusd_name()))?;
        if p.curves[..i].iter().any(|d| d.fan == c.fan) { return Err(format!("{} fan curve given twice", c.fan.asusd_name())); }
    }
    limits::validate(&p.settings, b)
}
```

Check the import paths against the codebase (`crate::hw::with_retry`, `crate::features::perf::HwCtx`) and adjust to match.

- [ ] **Step 5: Run the tests.** `cargo test -p armouryd manual::`. Expected: PASS. Then `cargo test --workspace`. Expected: green apart from the `#[ignore]`d tests.

- [ ] **Step 6: Commit.**

```bash
git add crates && git commit -m "feat(armouryd): stock and manual apply sequences"
```

---

### Task 3: Daemon: Target apply loop, manual requests, CLI

**Files:**
- Modify: `crates/proto/src/lib.rs` (requests), `crates/armouryd/src/ipc.rs`, `crates/armouryd/src/config.rs` (remove shim), `crates/armoury/src/main.rs`

**Interfaces:**
- Consumes: Task 1 types, Task 2 functions.
- Produces (proto `Request`):
  - Changed: `SetProfile { profile: ModeChoice }`, `NextProfile`
  - New: `LimitBounds`, `DefaultCurves { base: Profile }`, `ManualProfiles`, `SaveManualProfile { profile: ManualProfile, #[serde(default)] original_name: Option<String> }`, `ActivateManualProfile { name: String }`, `DeleteManualProfile { name: String }`
  - Removed: `FanCurves`, `SetFanCurve`, `ResetFanCurves`, `ModeSettings`, `SetModeSettings`
- Daemon methods:
  - `choose_mode(&self, c: ModeChoice) -> anyhow::Result<()>`
  - `ensure_manual_profile(&self)`
  - `reapply_manual(&self) -> Vec<String>`
  - `manual_view(&self) -> ManualView`
  - Snapshot: `perf.mode`, `perf.manual_profile`

- [ ] **Step 1: Change the proto requests.** Edit `Request` as listed. Update the proto round-trip tests that referenced removed variants: delete those asserts, and add:

```rust
    #[test]
    fn manual_requests_parse() {
        let r: Request = serde_json::from_str(r#"{"cmd":"set_profile","profile":"manual"}"#).unwrap();
        assert_eq!(r, Request::SetProfile { profile: ModeChoice::Manual });
        let r: Request = serde_json::from_str(r#"{"cmd":"save_manual_profile","profile":{"name":"G","base":"quiet"},"original_name":"F"}"#).unwrap();
        assert!(matches!(r, Request::SaveManualProfile { ref original_name, .. } if original_name.as_deref() == Some("F")));
        let r: Request = serde_json::from_str(r#"{"cmd":"activate_manual_profile","name":"G"}"#).unwrap();
        assert_eq!(r, Request::ActivateManualProfile { name: "G".into() });
    }
```

- [ ] **Step 2: Add the test helpers and write failing daemon tests.** In the `ipc.rs` tests module, add a rig whose asusd mirrors profile changes, and helpers:

```rust
    fn mrig(active: bool, toml: &str) -> Rig {
        let r = rig(active);
        std::fs::write(r.dir.path().join("config.toml"), toml).unwrap();
        let (cfg, _) = crate::config::Config::load(&r.dir.path().join("config.toml"));
        *r.d.config.try_lock().unwrap() = cfg;
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        r
    }
    fn fw(r: &Rig, p: &str) { r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), p.into()); }
    fn acalls_of(r: &Rig) -> Vec<String> { r.asusd.calls.lock().unwrap().clone() }
    fn clear(r: &Rig) { r.asusd.calls.lock().unwrap().clear(); r.svc.calls.lock().unwrap().clear(); }

    const GAMING: &str = r#"
[manual]
enabled = false
active = "Gaming"
[[manual.profiles]]
name = "Gaming"
base = "performance"
settings = { pl1 = 20, pl2 = 40 }
[[manual.profiles.curves]]
fan = "cpu"
temps = [30, 40, 50, 60, 70, 80, 90, 100]
percent = [0, 10, 20, 35, 55, 75, 90, 100]
"#;

    fn set(p: &str) -> Request { sreq(serde_json::json!({"cmd":"set_profile","profile":p})) }
```

(If `Daemon.config` isn't reachable from tests as `r.d.config`, it already is in this module: existing tests use `r.d.config.lock()`. Use `try_lock` only because the rig is idle.)

Tests:

```rust
    #[tokio::test]
    async fn stock_mode_turns_curves_off() {
        let r = mrig(true, "");
        r.d.tick().await; // firmware balanced
        assert!(acalls_of(&r).contains(&"set_fan_curves_enabled 0 false".to_string()), "{:?}", acalls_of(&r));
        clear(&r);
        r.d.tick().await;
        assert!(acalls_of(&r).is_empty(), "applied once, not every tick: {:?}", acalls_of(&r));
    }

    #[tokio::test]
    async fn entering_manual_applies_the_profile_and_stays() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        clear(&r);
        let resp = r.d.handle(set("manual")).await;
        assert!(resp.ok, "{resp:?}");
        let calls = acalls_of(&r);
        assert_eq!(&calls[..3], ["set_profile 1", "set_fan_curve 1 CPU", "set_fan_curves_enabled 1 true"], "{calls:?}");
        assert!(limit_calls(&r).last().unwrap().ends_with("set-limits pl1=20 pl2=40 cpu_boost=on"));
        clear(&r);
        r.d.tick().await; // our own switch to the base (balanced → performance) is not an outside change
        assert!(acalls_of(&r).is_empty(), "{:?}", acalls_of(&r));
        let s = r.d.refresh().await;
        assert_eq!((s.perf.mode, s.perf.manual_profile.as_deref()), (Some(ModeChoice::Manual), Some("Gaming")));
        assert!(r.d.config.lock().await.manual.enabled);
    }

    #[tokio::test]
    async fn outside_mode_change_leaves_manual() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        fw(&r, "quiet"); // e.g. asusd's own switch
        r.d.tick().await;
        assert!(acalls_of(&r).contains(&"set_fan_curves_enabled 2 false".to_string()), "{:?}", acalls_of(&r));
        assert!(!r.d.config.lock().await.manual.enabled);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Quiet));
    }

    #[tokio::test]
    async fn takeover_re_enters_manual() {
        let r = mrig(true, &GAMING.replace("enabled = false", "enabled = true"));
        r.d.tick().await; // first active tick (firmware still balanced)
        assert_eq!(acalls_of(&r)[0], "set_profile 1", "{:?}", acalls_of(&r));
        assert!(r.d.config.lock().await.manual.enabled, "re-entering is not an outside change");
    }

    #[tokio::test]
    async fn stock_choice_leaves_manual() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        assert!(r.d.handle(set("performance")).await.ok); // same firmware mode as the base
        assert!(acalls_of(&r).contains(&"set_fan_curves_enabled 1 false".to_string()), "{:?}", acalls_of(&r));
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Performance));
    }

    #[tokio::test]
    async fn next_profile_cycles_through_manual() {
        let r = mrig(true, GAMING);
        fw(&r, "performance");
        r.d.tick().await;
        assert!(r.d.handle(Request::NextProfile).await.ok);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Manual));
        assert!(r.d.handle(Request::NextProfile).await.ok);
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Quiet));
    }

    #[tokio::test]
    async fn first_manual_entry_creates_a_profile_from_turbo_curves() {
        let r = mrig(true, "");
        r.asusd.curves.lock().unwrap().insert(1, vec![("CPU".into(), [0, 25, 51, 76, 102, 153, 204, 255], [30, 40, 50, 60, 70, 80, 90, 100], true)]);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        let m = r.d.config.lock().await.manual.clone();
        assert_eq!((m.active.as_deref(), m.profiles.len()), (Some("Manual 1"), 1));
    }

    #[tokio::test]
    async fn saving_the_active_profile_reapplies_it() {
        let r = mrig(true, GAMING);
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        let mut p = r.d.config.lock().await.manual.profiles[0].clone();
        p.settings.pl1 = Some(30);
        let resp = r.d.handle(Request::SaveManualProfile { profile: p, original_name: Some("Gaming".into()) }).await;
        assert!(resp.ok, "{resp:?}");
        assert!(limit_calls(&r).last().unwrap().contains("pl1=30"), "{:?}", limit_calls(&r));
        let bad = { let mut q = r.d.config.lock().await.manual.profiles[0].clone(); q.settings.pl1 = Some(90); q.settings.pl2 = Some(50); q };
        assert!(!r.d.handle(Request::SaveManualProfile { profile: bad, original_name: None }).await.ok);
    }

    #[tokio::test]
    async fn activate_other_profile_switches_base() {
        let r = mrig(true, &format!("{GAMING}\n[[manual.profiles]]\nname = \"Quiet work\"\nbase = \"quiet\"\n"));
        r.d.tick().await;
        assert!(r.d.handle(set("manual")).await.ok);
        clear(&r);
        assert!(r.d.handle(Request::ActivateManualProfile { name: "Quiet work".into() }).await.ok);
        assert_eq!(acalls_of(&r)[0], "set_profile 2", "{:?}", acalls_of(&r));
        r.d.tick().await;
        assert!(r.d.config.lock().await.manual.enabled, "switching base for a new profile is ours");
    }

    #[tokio::test]
    async fn manual_writes_refused_in_observe_but_listing_works() {
        let r = mrig(false, GAMING);
        assert!(!r.d.handle(set("manual")).await.ok);
        assert!(!r.d.handle(Request::ActivateManualProfile { name: "Gaming".into() }).await.ok);
        let v = r.d.handle(Request::ManualProfiles).await;
        assert!(v.ok && v.data.unwrap()["profiles"][0]["name"] == "Gaming");
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Balanced), "Manual shows only while active");
    }
```

Then rewrite or delete each `#[ignore = "rewritten in Task 3"]` test. Find them with `grep -n 'rewritten in Task 3' crates/armouryd/src/ipc.rs`. Rule for each:
  - If it asserted per-mode limits/EPP/undervolt/NVIDIA being applied, rewrite it on `mrig` with the value in a manual profile's `settings`, entered via `set("manual")`. For example, `external_profile_change_applies_mode` becomes `outside_mode_change_leaves_manual`, which is above, so delete the old one.
  - If it tested `set_mode_settings` saving/validation, delete it; `saving_the_active_profile_reapplies_it` covers both.
  - If it tested fan-curve requests, delete it; curves are covered by `entering_manual_applies_the_profile_and_stays`.
  - Undervolt probe tests: move `uv_mv` into a manual profile.
  - `set_profile_reports_apply_failure`: keep; change `Request::SetProfile { profile: Profile::Balanced }` to `ModeChoice::Balanced`.
  - No `#[ignore]` may remain at the end of this task.

- [ ] **Step 3: Run and confirm they fail.** `cargo test -p armouryd ipc::` shows compile errors or failures on the new tests.

- [ ] **Step 4: Implement the apply loop.** In `ipc.rs`:

```rust
/// What the apply loop keeps in force.
#[derive(Debug, Clone, PartialEq)]
enum Target { Stock(Profile), Manual { name: String, base: Profile } }
```

`ApplyState.applied` and `.failed` become `Option<Target>`. In `tick_locked`, replace everything from `let Some(profile) = snap.perf.profile else ...` through the end with:

```rust
        let Some(fw) = snap.perf.profile else { return Vec::new() };
        let dgpu_active = snap.gpu.dgpu_active == Some(true);
        let woke = dgpu_active && !st.dgpu_was_active;
        st.dgpu_was_active = dgpu_active;
        let (target, manual) = self.resolve_target(fw, st).await;
        let settings = manual.as_ref().map(|p| p.settings).unwrap_or_default();
        let base = manual.as_ref().map_or(fw, |p| p.base);
        let secs = self.config.lock().await.reapply_power_secs;
        let due = secs > 0 && now.duration_since(*self.last_reapply.lock().unwrap()) >= Duration::from_secs(secs as u64);
        if st.applied.as_ref() == Some(&target) {
            let mut errs = Vec::new();
            if woke && !(nv_is_stock(&settings) && st.nv_at_stock) {
                errs = apply_nv(&settings, &*self.svc).await;
                st.nv_at_stock = errs.is_empty() && nv_is_stock(&settings);
            }
            if due {
                errs.extend(apply_mode(base, &settings, HwCtx { limits_only: true, ..self.hw_ctx(dgpu_active, st) }, &*self.asusd, &*self.svc).await);
                *self.last_reapply.lock().unwrap() = now;
            }
            self.note_apply(&target, &errs);
            return errs;
        }
        if !force && st.failed.as_ref() == Some(&target) && st.retry_at.is_some_and(|t| now < t) { return Vec::new(); }
        let ctx = self.hw_ctx(dgpu_active, st);
        let errs = match &manual {
            Some(p) => manual::apply_manual(p, ctx, &*self.asusd, &*self.svc).await,
            None => manual::apply_stock(fw, ctx, &*self.asusd, &*self.svc).await,
        };
        self.note_apply(&target, &errs);
        if errs.is_empty() {
            *st = ApplyState {
                applied: Some(target),
                dgpu_was_active: dgpu_active,
                uv_at_stock: if ctx.uv_unlocked { settings.uv_mv.unwrap_or(0) == 0 } else { st.uv_at_stock },
                nv_at_stock: if dgpu_active { nv_is_stock(&settings) } else { st.nv_at_stock },
                probe_retry_at: st.probe_retry_at,
                ..Default::default()
            };
            *self.last_reapply.lock().unwrap() = now;
        } else {
            *st = ApplyState { failed: Some(target), retry_at: Some(now + RETRY_BACKOFF), dgpu_was_active: dgpu_active, probe_retry_at: st.probe_retry_at, ..Default::default() };
        }
        errs
```

Add:

```rust
    /// Manual when it's on, unless the firmware mode was changed away from the applied
    /// manual base by something else: then Manual is switched off and that stock mode is kept.
    async fn resolve_target(&self, fw: Profile, st: &ApplyState) -> (Target, Option<ManualProfile>) {
        // one lock per statement: two guards in one expression would deadlock
        let needs_profile = { let c = self.config.lock().await; c.manual.enabled && c.manual.profiles.is_empty() };
        if needs_profile { self.ensure_manual_profile().await; }
        let mut cfg = self.config.lock().await;
        if !cfg.manual.enabled { return (Target::Stock(fw), None); }
        if let Some(Target::Manual { base, .. }) = &st.applied {
            if *base != fw {
                cfg.manual.enabled = false;
                if let Err(e) = cfg.save(&self.config_path) { eprintln!("armouryd: save config: {e}"); }
                eprintln!("armouryd: mode changed to {} outside armouryd; leaving Manual", fw.sysfs());
                return (Target::Stock(fw), None);
            }
        }
        match cfg.manual.active_profile().cloned() {
            Some(p) => (Target::Manual { name: p.name.clone(), base: p.base }, Some(p)),
            None => (Target::Stock(fw), None),
        }
    }

    /// Creates "Manual 1" from asusd's current Turbo curves when no profile exists yet.
    async fn ensure_manual_profile(&self) {
        if !self.config.lock().await.manual.profiles.is_empty() { return; }
        let raw = with_retry(|| self.asusd.fan_curves(Profile::Performance.to_asusd())).await.unwrap_or_default();
        let mut cfg = self.config.lock().await;
        if cfg.manual.profiles.is_empty() {
            let _ = cfg.manual.save(manual::default_profile("Manual 1", &raw), None);
            if let Err(e) = cfg.save(&self.config_path) { eprintln!("armouryd: save config: {e}"); }
        }
    }

    /// The user picked a mode (UI, CLI, mode key, power source). Never holds the apply lock.
    async fn choose_mode(&self, choice: ModeChoice) -> anyhow::Result<()> {
        match choice.stock() {
            Some(p) => {
                {
                    let mut cfg = self.config.lock().await;
                    if cfg.manual.enabled { cfg.manual.enabled = false; cfg.save(&self.config_path)?; }
                }
                with_retry(|| self.asusd.set_profile(p.to_asusd())).await?;
            }
            None => {
                self.ensure_manual_profile().await;
                let mut cfg = self.config.lock().await;
                cfg.manual.enabled = true;
                cfg.save(&self.config_path)?;
            }
        }
        Ok(())
    }

    /// Re-applies Manual from scratch (profile edited, activated or deleted).
    async fn reapply_manual(&self) -> Vec<String> {
        let mut st = self.apply.lock().await;
        st.applied = None;
        st.failed = None;
        self.tick_locked(&mut st, true).await
    }

    async fn manual_view(&self) -> ManualView {
        let m = self.config.lock().await.manual.clone();
        let active = m.active_profile().map(|p| p.name.clone());
        ManualView { enabled: m.enabled, active, profiles: m.profiles }
    }
```

`note_apply` takes `&Target`. The label is `Stock(p) => p.sysfs()`, `Manual { name, .. } => format!("manual ({name})")`:

```rust
    fn note_apply(&self, target: &Target, errs: &[String]) {
        let label = match target { Target::Stock(p) => p.sysfs().to_string(), Target::Manual { name, .. } => format!("manual ({name})") };
        for e in errs { eprintln!("armouryd: apply {label}: {e}"); }
        *self.apply_error.lock().unwrap() = (!errs.is_empty()).then(|| format!("{label}: {}", errs.join("; ")));
    }
```

In `snapshot()`, after the existing field fills:

```rust
        {
            let cfg = self.config.lock().await;
            s.perf.mode = if mode == ControlMode::Active && cfg.manual.enabled { Some(ModeChoice::Manual) } else { s.perf.profile.map(ModeChoice::from) };
            s.perf.manual_profile = cfg.manual.active_profile().map(|p| p.name.clone());
        }
```

- [ ] **Step 5: Implement the request handlers.** Replace the `SetProfile` and `NextProfile` arms, and delete the `FanCurves`, `SetFanCurve`, `ResetFanCurves`, `ModeSettings` and `SetModeSettings` arms and the `curves()` helper:

```rust
            Request::SetProfile { profile } => {
                if let Err(r) = self.write_guard().await { return r; }
                self.snapshot_after(self.choose_mode(profile).await).await
            }
            Request::NextProfile => {
                if let Err(r) = self.write_guard().await { return r; }
                let cur = self.refresh().await.perf.mode;
                let i = cur.and_then(|c| ModeChoice::CYCLE.iter().position(|m| *m == c)).map_or(0, |i| (i + 1) % 4);
                self.snapshot_after(self.choose_mode(ModeChoice::CYCLE[i]).await).await
            }
            Request::LimitBounds => {
                let b = self.bounds().await;
                let bounds: serde_json::Map<String, serde_json::Value> =
                    Limit::ALL.iter().map(|l| (l.key().to_string(), serde_json::json!([b(*l).min, b(*l).max]))).collect();
                Response::ok(serde_json::Value::Object(bounds))
            }
            Request::ManualProfiles => Response::ok(serde_json::to_value(self.manual_view().await).unwrap()),
            Request::DefaultCurves { base } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = with_retry(|| self.asusd.reset_fan_curves(base.to_asusd())).await { return Response::err(format!("{e:#}")); }
                let raw = match with_retry(|| self.asusd.fan_curves(base.to_asusd())).await { Ok(r) => r, Err(e) => return Response::err(format!("{e:#}")) };
                // the reset also cleared Manual's curves if they live in that slot
                let in_slot = { let c = self.config.lock().await; c.manual.enabled && c.manual.active_profile().is_some_and(|p| p.base == base) };
                if in_slot { self.reapply_manual().await; }
                Response::ok(serde_json::to_value(raw.iter().filter_map(fan::from_raw).collect::<Vec<_>>()).unwrap())
            }
            Request::SaveManualProfile { profile, original_name } => {
                if let Err(r) = self.write_guard().await { return r; }
                if let Err(e) = manual::validate_profile(&profile, self.bounds().await) { return Response::err(e); }
                let reapply = {
                    let mut cfg = self.config.lock().await;
                    let was_active = cfg.manual.active_profile().map(|p| p.name.clone());
                    if let Err(e) = cfg.manual.save(profile.clone(), original_name.as_deref()) { return Response::err(e); }
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                    cfg.manual.enabled && (was_active == original_name.or(Some(profile.name.clone())))
                };
                if reapply {
                    let errs = self.reapply_manual().await;
                    if !errs.is_empty() { return Response::err(format!("saved, but applying failed: {}", errs.join("; "))); }
                }
                Response::ok(serde_json::to_value(self.manual_view().await).unwrap())
            }
            Request::ActivateManualProfile { name } => {
                if let Err(r) = self.write_guard().await { return r; }
                {
                    let mut cfg = self.config.lock().await;
                    if let Err(e) = cfg.manual.activate(&name) { return Response::err(e); }
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                }
                let errs = self.reapply_manual().await;
                if !errs.is_empty() { return Response::err(format!("activated, but applying failed: {}", errs.join("; "))); }
                Response::ok(serde_json::to_value(self.manual_view().await).unwrap())
            }
            Request::DeleteManualProfile { name } => {
                if let Err(r) = self.write_guard().await { return r; }
                let reapply = {
                    let mut cfg = self.config.lock().await;
                    let was_active = cfg.manual.active_profile().is_some_and(|p| p.name == name);
                    if let Err(e) = cfg.manual.delete(&name) { return Response::err(e); }
                    if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                    was_active && cfg.manual.enabled
                };
                if reapply { self.reapply_manual().await; }
                Response::ok(serde_json::to_value(self.manual_view().await).unwrap())
            }
```

Add `ModeChoice, ManualProfile, ManualView` to the proto imports and `use crate::features::manual;`. Delete the Task 1 `Config::mode` shim, and update the uv-probe condition if it isn't already updated.

- [ ] **Step 6: Mode key OSD.** In `on_hotkey`, in the `KeyAction::CycleMode` arm, replace the label lookup with:

```rust
                let s = self.refresh().await;
                let mode = match s.perf.mode {
                    Some(ModeChoice::Manual) => format!("Manual ({})", s.perf.manual_profile.unwrap_or_default()),
                    Some(m) => m.label().to_string(),
                    None => "Unknown".into(),
                };
                self.osd(&if r.ok { format!("{mode} mode") } else { format!("{mode} mode (settings failed)") }).await;
```

- [ ] **Step 7: Update the CLI.** In `crates/armoury/src/main.rs`:
  - `ProfileAction::Set { #[arg(value_parser = parse_mode_choice)] profile: ModeChoice }`
  - Print `s.perf.mode.map(|m| m.label()).unwrap_or("-")`
  - Remove the `Fan` and `Mode` commands, `FanAction`, `ModeAction` and `parse_curve` (and its tests), plus unused imports
  - Add:

```rust
    /// Manual-mode profiles
    Manual {
        #[command(subcommand)]
        action: Option<ManualAction>,
    },
```

```rust
#[derive(Subcommand)]
enum ManualAction {
    /// Make a profile active and switch to Manual
    Activate { name: String },
}
```

```rust
        Cmd::Manual { action } => {
            let req = match action {
                None => Request::ManualProfiles,
                Some(ManualAction::Activate { name }) => Request::ActivateManualProfile { name },
            };
            let v: ManualView = serde_json::from_value(call(&req)?)?;
            for p in v.profiles {
                let mark = if v.active.as_deref() == Some(p.name.as_str()) { if v.enabled { "* " } else { "- " } } else { "  " };
                println!("{mark}{} (on {})", p.name, ModeChoice::from(p.base).label());
            }
        }
```

  (`*` = active and in use; `-` = active but a stock mode is on.) Match the derive name used by the file's other subcommand enums (`Subcommand`).

- [ ] **Step 8: Run everything.** `cargo test --workspace` then `cargo clippy --workspace 2>&1 | grep -c warning`. Expected: all tests PASS, no `#[ignore]` left (`grep -c 'rewritten in Task 3' crates/armouryd/src/ipc.rs` prints 0), and no new warnings compared with before the task.

- [ ] **Step 9: Commit.**

```bash
git add crates && git commit -m "feat(armouryd): Manual mode — profiles applied on their base mode, stock modes on firmware auto; CLI 'armoury manual'"
```

---

### Task 4: Power-source modes switched by armouryd

**Files:**
- Modify: `crates/armouryd/src/ipc.rs`, `crates/armouryd/src/hw/mod.rs`, `crates/armouryd/src/hw/asusd.rs`, `crates/armouryd/src/hw/fake.rs`

**Interfaces:**
- Consumes: `choose_mode`, `SysSource`.
- Produces:
  - `Asusd::disable_source_switching(&self) -> anyhow::Result<()>`, replacing `set_source_profiles`
  - `SysSource.mode_on_ac: Option<bool>`
  - `follow_source_mode(&self, snap: &Snapshot) -> bool`

- [ ] **Step 1: Write failing tests.** Replace `source_profiles_go_to_asusd` in `ipc.rs` tests with:

```rust
    #[tokio::test]
    async fn source_profiles_saved_and_asusd_switching_disabled() {
        let r = sys_rig(true, "");
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_source_profile","ac":"manual","battery":"quiet"}))).await.ok);
        assert_eq!(*r.asusd.calls.lock().unwrap(), ["disable_source_switching"]);
        let cfg = r.d.config.lock().await.system.clone();
        assert_eq!((cfg.profile_ac, cfg.profile_battery), (Some(ModeChoice::Manual), Some(ModeChoice::Quiet)));
    }

    #[tokio::test]
    async fn power_source_flip_switches_mode() {
        let r = sys_rig(true, &format!("[system]\nprofile_ac = \"manual\"\nprofile_battery = \"quiet\"\n{GAMING}"));
        *r.asusd.mirror.lock().unwrap() = Some(r.sys.clone());
        r.sys.files.lock().unwrap().insert("sys/firmware/acpi/platform_profile".into(), "balanced".into());
        r.d.tick().await; // first tick on AC: no switch
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Balanced));
        plug(&r, false);
        r.d.tick().await;
        assert_eq!(r.d.refresh().await.perf.mode, Some(ModeChoice::Quiet));
        plug(&r, true);
        r.d.tick().await;
        let s = r.d.refresh().await;
        assert_eq!(s.perf.mode, Some(ModeChoice::Manual));
        assert!(r.asusd.calls.lock().unwrap().iter().any(|c| c == "set_fan_curve 1 CPU"), "manual applied in the same tick");
    }
```

(`GAMING` is the Task 3 constant; move it to module scope in the tests module if it isn't already. `sys_rig` doesn't set a platform profile, so the test inserts one.)

- [ ] **Step 2: Run and confirm they fail.** `cargo test -p armouryd source_profiles power_source_flip`. Expected: FAIL.

- [ ] **Step 3: Implement.**
  - **Trait:** in `hw/mod.rs`, rename the method to `async fn disable_source_switching(&self) -> anyhow::Result<()>;` (plus the forwarding impl).
  - **asusd:** in `hw/asusd.rs`:

```rust
    async fn disable_source_switching(&self) -> anyhow::Result<()> {
        // armouryd switches modes on AC/battery itself (asusd doesn't know Manual)
        let p = self.platform().await?;
        p.set_change_platform_profile_on_ac(false).await?;
        p.set_change_platform_profile_on_battery(false).await?;
        Ok(())
    }
```

  - **Fake:** `async fn disable_source_switching(&self) -> anyhow::Result<()> { self.record("disable_source_switching".into()) }`
  - **Handler:** in the `SetSourceProfile` arm:

```rust
                Request::SetSourceProfile { ac, battery } => {
                    with_retry(|| self.asusd.disable_source_switching()).await?;
                    let mut cfg = self.config.lock().await;
                    if ac.is_some() { cfg.system.profile_ac = ac; }
                    if battery.is_some() { cfg.system.profile_battery = battery; }
                    cfg.save(&self.config_path)?;
                    Ok(serde_json::json!({"ac": ac, "battery": battery}))
                }
```

  - **State:** add `mode_on_ac: Option<bool>,` to `SysSource` with the doc comment "Power source seen by the mode switch (None until the first active tick)".
  - **Flip detection:** add:

```rust
    /// On a real AC↔battery flip, enter the mode configured for the new source.
    /// The first tick after start/takeover only records the source.
    async fn follow_source_mode(&self, snap: &Snapshot) -> bool {
        let Some(ac) = snap.lighting.on_ac else { return false };
        let prev = self.sys_src.lock().unwrap().mode_on_ac.replace(ac);
        if prev != Some(!ac) { return false; }
        let want = { let c = self.config.lock().await; if ac { c.system.profile_ac } else { c.system.profile_battery } };
        let Some(choice) = want else { return false };
        match self.choose_mode(choice).await {
            Ok(()) => true,
            Err(e) => { eprintln!("armouryd: power-source mode: {e:#}"); false }
        }
    }
```

  - **Tick:** in `tick_locked`, right after `self.follow_system(&snap).await;`:

```rust
        let snap = if self.follow_source_mode(&snap).await { self.refresh().await } else { snap };
```

- [ ] **Step 4: Run the tests.** `cargo test --workspace`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates && git commit -m "feat(armouryd): AC/battery mode switching (incl. Manual) done by armouryd; asusd's own switching disabled"
```

---

### Task 5: Plugin: Manual page, fourth mode, power-source rows

**Files:**
- Create: `plugin/ManualPage.qml`
- Delete: `plugin/FansPage.qml`, `plugin/CpuGpuPage.qml`
- Modify: `plugin/Window.qml`, `plugin/Widget.qml`, `plugin/SystemPage.qml`

**Interfaces:**
- Consumes these requests: `manual_profiles`, `limit_bounds`, `save_manual_profile`, `activate_manual_profile`, `delete_manual_profile`, `default_curves`, `set_profile` (`manual` allowed), `status`, and `snap.perf.mode` / `snap.perf.manual_profile`.
- Reuses: `FanGraph`, `ValueSlider` (`live`/`committed`), `ChoiceRow`, `Section`, `InfoRow`, `WheelScroll`.

- [ ] **Step 1: Create `plugin/ManualPage.qml`.**

```qml
import QtQuick
import qs.Commons
import qs.Ui

// Manual mode: saved profiles, each a base mode + fan curves + CPU/GPU tuning.
// Fan graphs keep their own edits; `curveModel` is set only when a profile is
// selected, so changing a slider never rebuilds the graphs mid-edit.
Flickable {
  id: root
  property var client: null
  property color fg: Color.foreground
  property string fontFamily: Style.font.family

  readonly property var snap: client ? client.snap : null
  readonly property bool usable: !!(client && client.active)
  readonly property bool manualOn: !!(snap && snap.perf && snap.perf.mode === "manual")
  property var view: ({ enabled: false, active: null, profiles: [] })
  property var bounds: ({})
  property string selected: ""
  property string base: "performance"
  property var settings: ({})
  property var curveModel: []
  property bool dirty: false
  property bool renaming: false
  property var detail: null

  contentWidth: width
  contentHeight: col.implicitHeight
  clip: true
  interactive: false   // drags belong to sliders and graphs; WheelScroll scrolls
  WheelScroll { flick: root }

  function copy(o) { return JSON.parse(JSON.stringify(o)) }
  function find(name) {
    for (var i = 0; i < view.profiles.length; i++) if (view.profiles[i].name === name) return view.profiles[i]
    return null
  }
  function select(name) {
    var p = find(name)
    if (!p) return
    selected = name; base = p.base; settings = copy(p.settings || {}); curveModel = copy(p.curves || [])
    dirty = false; renaming = false
  }
  function load(keep) {
    client.call({ cmd: "manual_profiles" }, function(r) {
      if (!r.ok) return
      root.view = r.data
      var want = keep && root.find(keep) ? keep : (r.data.active || (r.data.profiles.length ? r.data.profiles[0].name : ""))
      if (want) root.select(want)
    })
    client.call({ cmd: "limit_bounds" }, function(r) { if (r.ok) root.bounds = r.data })
  }
  Component.onCompleted: load("")

  function current(name) {
    var curves = []
    for (var k = 0; k < fanRepeater.count; k++) {
      var g = fanRepeater.itemAt(k).graph
      curves.push({ fan: curveModel[k].fan, temps: g.temps.slice(), percent: g.percent.slice() })
    }
    return { name: name, base: base, curves: curves, settings: settings }
  }
  function saveAs(name, original, then) {
    client.run({ cmd: "save_manual_profile", profile: current(name), original_name: original }, function(r) {
      if (!r.ok) return
      root.load(name)
      if (then) then()
    })
  }
  function uniqueName() {
    for (var n = 1; ; n++) if (!find("Manual " + n)) return "Manual " + n
  }
  function val(key, fallback) { return settings[key] !== undefined ? settings[key] : fallback }
  function set(key, v) { var s = copy(settings); s[key] = v; settings = s; dirty = true }
  function range(key, lo, hi) { return bounds[key] || [lo, hi] }
  readonly property bool isActive: view.active === selected && manualOn

  // NVIDIA details only while the page is open, every 5 s: reading them keeps the dGPU awake.
  Timer {
    interval: 5000
    running: !!(root.visible && root.snap && root.snap.gpu && root.snap.gpu.dgpu_active === true)
    repeat: true
    triggeredOnStart: true
    onTriggered: root.client.call({ cmd: "status" }, function(r) { if (r.ok) root.detail = r.data.gpu })
  }

  Column {
    id: col
    width: Math.min(root.width, Style.space(680))
    spacing: Style.space(14)

    Text {
      visible: !root.manualOn
      width: parent.width
      wrapMode: Text.WordWrap
      text: "Fans are on firmware auto in Silent, Balanced and Turbo. Activate a profile to use it."
      color: root.fg; opacity: 0.7; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }

    // ---------- profile picker ----------
    Row {
      width: parent.width
      spacing: Style.space(8)
      Dropdown {
        id: picker
        width: parent.width - newBtn.width - renameBtn.width - deleteBtn.width - parent.spacing * 3
        label: "Profile"
        options: root.view.profiles.map(function(p) {
          return { label: p.name + (p.name === root.view.active ? (root.manualOn ? "  · in use" : "  · active") : ""), value: p.name }
        })
        value: root.selected
        onChanged: function(v) { root.select(v) }
      }
      Button {
        id: newBtn; anchors.bottom: parent.bottom; text: "New"; bordered: true; foreground: root.fg; enabled: root.usable
        onClicked: root.saveAs(root.uniqueName(), null)
      }
      Button {
        id: renameBtn; anchors.bottom: parent.bottom; text: "Rename"; foreground: root.fg; enabled: root.usable && root.selected !== ""
        onClicked: { nameField.text = root.selected; root.renaming = true }
      }
      Button {
        id: deleteBtn; anchors.bottom: parent.bottom; text: "Delete"; foreground: root.fg
        enabled: root.usable && root.view.profiles.length > 1
        onClicked: root.client.run({ cmd: "delete_manual_profile", name: root.selected }, function(r) { if (r.ok) root.load("") })
      }
    }
    Row {
      visible: root.renaming
      width: parent.width
      spacing: Style.space(8)
      TextField { id: nameField; width: parent.width - renameSave.width - parent.spacing; foreground: root.fg }
      Button {
        id: renameSave; text: "Save name"; bordered: true; foreground: root.fg; enabled: root.usable && nameField.text.trim() !== ""
        onClicked: root.saveAs(nameField.text.trim(), root.selected)
      }
    }

    ChoiceRow {
      fg: root.fg
      label: "Runs on"
      usable: root.usable
      options: [{ label: "Silent", value: "quiet" }, { label: "Balanced", value: "balanced" }, { label: "Turbo", value: "performance" }]
      value: root.base
      onChosen: function(v) { root.base = v; root.dirty = true }
    }

    // ---------- fans ----------
    Section { text: "FANS"; fg: root.fg }
    Repeater {
      id: fanRepeater
      model: root.curveModel
      Column {
        id: fanCol
        required property var modelData
        required property int index
        property alias graph: graph
        width: col.width
        spacing: Style.space(4)
        Text {
          text: (fanCol.modelData.fan === "gpu" ? "GPU fan" : fanCol.modelData.fan === "mid" ? "Mid fan" : "CPU fan")
            + (fanCol.index === 0 && root.snap && root.snap.perf ? "  ·  " + (root.snap.perf.cpu_fan_rpm || 0) + " rpm" : "")
            + (fanCol.index === 1 && root.snap && root.snap.perf ? "  ·  " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm" : "")
          color: root.fg; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall; font.bold: true
        }
        FanGraph {
          id: graph
          width: parent.width
          height: Style.space(190)
          temps: fanCol.modelData.temps
          percent: fanCol.modelData.percent
          lineColor: fanCol.index === 0 ? Color.accent : Qt.lighter(Color.urgent, 1.1)
          fg: root.fg
          usable: root.usable
          onEdited: root.dirty = true
        }
      }
    }
    Row {
      spacing: Style.space(10)
      Text {
        anchors.verticalCenter: parent.verticalCenter
        text: "Drag points to edit · Shift-drag moves the whole curve"
        color: root.fg; opacity: 0.55; font.family: root.fontFamily; font.pixelSize: Style.font.caption
      }
      Button {
        text: "Firmware curves"; fontSize: Style.font.caption; foreground: root.fg; enabled: root.usable
        onClicked: root.client.run({ cmd: "default_curves", base: root.base }, function(r) {
          if (r.ok) { root.curveModel = r.data; root.dirty = true }
        })
      }
    }

    // ---------- CPU ----------
    Section { text: "CPU"; fg: root.fg }
    ValueSlider {
      fg: root.fg; label: "PL1 (sustained)"; unit: "W"
      minimum: root.range("pl1", 5, 150)[0]; maximum: root.range("pl1", 5, 150)[1]
      value: root.val("pl1", 45); unset: root.val("pl1", undefined) === undefined
      usable: root.usable
      // PL2 can never sit below PL1: it slides up with it
      onLive: function(v) { if (v > root.val("pl2", 65)) root.set("pl2", v) }
      onCommitted: function(v) { root.set("pl1", v); if (v > root.val("pl2", 65)) root.set("pl2", v) }
    }
    ValueSlider {
      fg: root.fg; label: "PL2 (boost)"; unit: "W"
      minimum: root.range("pl2", 5, 150)[0]; maximum: root.range("pl2", 5, 150)[1]
      value: root.val("pl2", 65); unset: root.val("pl2", undefined) === undefined
      usable: root.usable
      // PL1 can never sit above PL2: it slides down with it
      onLive: function(v) { if (v < root.val("pl1", 45)) root.set("pl1", v) }
      onCommitted: function(v) { root.set("pl2", v); if (v < root.val("pl1", 45)) root.set("pl1", v) }
    }
    ChoiceRow {
      fg: root.fg; label: "CPU boost"; usable: root.usable
      options: [{ label: "On", value: true }, { label: "Off", value: false }]
      value: root.val("cpu_boost", true)
      onChosen: function(v) { root.set("cpu_boost", v) }
    }
    Dropdown {
      width: Math.min(parent.width, Style.space(320))
      label: "Energy preference (EPP)"
      options: [
        { label: "Default", value: "default" }, { label: "Performance", value: "performance" },
        { label: "Balance performance", value: "balance_performance" }, { label: "Balance power", value: "balance_power" },
        { label: "Power saving", value: "power" }
      ]
      value: root.val("epp", "default")
      enabled: root.usable
      onChanged: function(v) { root.set("epp", v) }
    }
    ValueSlider {
      visible: !!(root.snap && root.snap.perf && root.snap.perf.undervolt && root.snap.perf.undervolt.unlocked)
      fg: root.fg; label: "Undervolt (core + cache)"; unit: "mV"
      minimum: -150; maximum: 0
      value: root.val("uv_mv", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("uv_mv", v) }
    }

    // ---------- NVIDIA ----------
    Section { text: "NVIDIA"; fg: root.fg }
    ValueSlider {
      fg: root.fg; label: "Dynamic Boost"; unit: "W"
      minimum: root.range("nv_boost", 5, 25)[0]; maximum: root.range("nv_boost", 5, 25)[1]
      value: root.val("nv_boost", 25); unset: root.val("nv_boost", undefined) === undefined
      usable: root.usable
      onCommitted: function(v) { root.set("nv_boost", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Temperature target"; unit: "°C"
      minimum: root.range("nv_temp", 75, 87)[0]; maximum: root.range("nv_temp", 75, 87)[1]
      value: root.val("nv_temp", 87); unset: root.val("nv_temp", undefined) === undefined
      usable: root.usable
      onCommitted: function(v) { root.set("nv_temp", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Core clock offset"; unit: "MHz"
      minimum: -300; maximum: 300; step: 5
      value: root.val("gpu_core_offset", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("gpu_core_offset", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Memory clock offset"; unit: "MHz"
      minimum: -500; maximum: 1500; step: 10
      value: root.val("gpu_mem_offset", 0)
      usable: root.usable
      onCommitted: function(v) { root.set("gpu_mem_offset", v) }
    }

    // ---------- save / activate ----------
    Row {
      spacing: Style.space(10)
      Button {
        text: root.dirty ? (root.isActive ? "Save and apply" : "Save") : "Saved"
        bordered: true; foreground: root.fg
        enabled: root.usable && root.dirty && root.selected !== ""
        onClicked: root.saveAs(root.selected, root.selected)
      }
      Button {
        visible: !root.isActive && root.selected !== ""
        text: root.dirty ? "Save and activate" : "Activate"
        bordered: true; foreground: root.fg; enabled: root.usable
        onClicked: {
          var name = root.selected
          var go = function() { root.client.run({ cmd: "activate_manual_profile", name: name }, function(r) { if (r.ok) root.load(name) }) }
          if (root.dirty) root.saveAs(name, name, go); else go()
        }
      }
      Button { text: "Discard"; foreground: root.fg; visible: root.dirty; onClicked: root.select(root.selected) }
    }

    // ---------- GPU status ----------
    Section { text: "GPU STATUS"; fg: root.fg }
    Text {
      visible: !(root.snap && root.snap.gpu && root.snap.gpu.dgpu_active === true)
      text: "dGPU is asleep (saving power)."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.bodySmall
    }
    Column {
      visible: !!root.detail && !!root.detail.nvidia
      width: parent.width
      spacing: Style.spacing.labelGap
      readonly property var n: root.detail && root.detail.nvidia ? root.detail.nvidia : ({})
      InfoRow { fg: root.fg; label: "Clocks"; value: (parent.n.core_mhz || 0) + " / " + (parent.n.mem_mhz || 0) + " MHz" }
      InfoRow { fg: root.fg; label: "Temperature · power"; value: (parent.n.temp_c || 0) + "°C · " + Number(parent.n.power_w || 0).toFixed(1) + " W" }
      InfoRow { fg: root.fg; label: "Load · VRAM"; value: (parent.n.util_pct || 0) + "% · " + (parent.n.vram_used_mb || 0) + " / " + (parent.n.vram_total_mb || 0) + " MB" }
      InfoRow {
        fg: root.fg; label: "Keeping it awake"
        value: root.detail && root.detail.users && root.detail.users.length ? root.detail.users.map(function(u) { return u.name }).join(", ") : "—"
      }
    }
  }
}
```

- [ ] **Step 2: Wire the window.**
  - `git rm plugin/FansPage.qml plugin/CpuGpuPage.qml`.
  - In `plugin/Window.qml`, replace the Fans and CPU/GPU `Tile`s with one tile:

```qml
              Tile {
                pageId: "manual"; icon: "󰈐"; title: "Manual"
                lines: root.snap && root.snap.perf ? [
                  root.snap.perf.mode === "manual" ? "In use: " + (root.snap.perf.manual_profile || "—") : "Profile: " + (root.snap.perf.manual_profile || "none yet"),
                  "CPU " + (root.snap.perf.cpu_fan_rpm || 0) + " rpm · GPU " + (root.snap.perf.gpu_fan_rpm || 0) + " rpm",
                  "Fan curves, power limits, GPU"
                ] : []
              }
```

  - In the page `Loader`, replace the `fans` and `cpugpu` branches with `root.page === "manual" ? manualPage`. Replace the two page Components with `Component { id: manualPage; ManualPage { client: armoury; fg: root.fg; fontFamily: root.fontFamily } }`.
  - In `pageTitle`, replace `fans` and `cpugpu` with `manual: "Manual"`.
  - In `open()`, map the old payload names: `if (pg === "fans" || pg === "cpugpu") pg = "manual"` before the `pageTitle` check.
  - Search for other uses of `modeLabel(root.snap.perf.profile)` in `Window.qml` and switch them to `snap.perf.mode`, with Manual labelled "Manual".

- [ ] **Step 3: Fourth mode in the widget.**
  - In `plugin/Widget.qml`, add `{ id: "manual", label: "Manual", icon: "󰈐" }` to `modes`.
  - Change `readonly property string profile:` to read `snap.perf.mode`: `snap && snap.perf ? (snap.perf.mode || snap.perf.profile || "") : ""`.
  - The existing button row and `setMode(id)` then send `set_profile` with `manual`.
  - If the MODE row lays out buttons at a fixed third of the width, change the divisor to `root.modes.length`.

- [ ] **Step 4: Power-source rows.** In `plugin/SystemPage.qml`, change `modeOptions` to add `{ label: "Manual", value: "manual" }`, and change the caption text to "Switched automatically when you plug in or unplug. Manual uses the active manual profile."

- [ ] **Step 5: Compile check.** `./tests/qml_check.sh`. Expected: every file OK. `FansPage` and `CpuGpuPage` no longer appear.

- [ ] **Step 6: Runtime probe for Review Focus #3.** Create a probe shell in the scratchpad, as in the earlier probes: a `Scope` with a fake client whose `call` answers `manual_profiles` with one profile of two curves, and `limit_bounds` with `{}`. It should:
  1. Create `ManualPage`.
  2. Record `fanRepeater.itemAt(0).graph` into a variable `g0`.
  3. Call `p.set("pl1", 80)`, then `p.base = "quiet"`.
  4. Log `fanRepeater.itemAt(0).graph === g0`.
  5. Log `p.val("pl2", 65)` after `p.set("pl1", 100)` followed by the PL1 slider's live handler logic.

  The first check in step 4 relies on `curveModel` never changing on a settings edit. If `fanRepeater` isn't reachable from outside the file, expose `property alias fans: fanRepeater` on the page. Expected output: `true`.

- [ ] **Step 7: Commit.**

```bash
git add -A plugin && git commit -m "feat(ui): Manual page (profiles, base mode, curves, tuning), fourth mode in the popup, Manual for AC/battery"
```

---

### Task 6: Install and verify on the laptop

**Files:** none (verification); fixes go to the task that owns the code.

- [ ] **Step 1: Install.** `cargo build --release && install -Dm755 target/release/armouryd ~/.local/bin/armouryd && install -Dm755 target/release/armoury ~/.local/bin/armoury && systemctl --user restart armouryd && omarchy-restart-shell`. Then `armoury status` shows `Control observe`.

- [ ] **Step 2: Screenshots in observe.** Summon `{"page":"manual"}` and the dashboard, and look at them. Expected:
  - the "firmware auto" line
  - the profile picker empty, or showing existing profiles
  - all write controls disabled

- [ ] **Step 3: Takeover session.** Every check from here on happens during the takeover; Step 4 hands back.

  1. `armoury takeover`.
  2. `armoury profile set manual`. Expected: `Manual`; `armoury manual` lists `* Manual 1 (on Turbo)`.
  3. Edit that profile through the page (or `save_manual_profile` over the socket) to PL1 = PL2 = 20 W and apply.
  4. Run the Plan 2 load test (`stress-ng --cpu 20 --timeout 40s` while sampling package power from RAPL). Expected: package power holds ≈20 W.
  5. `armoury profile set performance`. Expected:
     - `busctl` / asusd reports Turbo curves disabled (`asusctl fan-curve --mode-get` or the fan_curves D-Bus read)
     - the load test is no longer capped at 20 W
  6. `armoury auto profile --ac manual --battery quiet`. The user unplugs the charger: the mode goes to Silent. The user plugs it back in: the mode goes to Manual.
  7. Press Fn+F5 four times: the OSD shows Silent → Balanced → Turbo → Manual (Gaming…) in order.

- [ ] **Step 4: Hand back.** `armoury handback`. Then `armoury status` shows `observe`, `systemctl is-enabled asusd` prints `masked`, and G-Helper is running.

- [ ] **Step 5: Final review and merge.** Run a whole-branch review (Plan 5 UI plus this plan) on the most capable model, fix its findings, and merge `plan-5-ui` into `main`.
