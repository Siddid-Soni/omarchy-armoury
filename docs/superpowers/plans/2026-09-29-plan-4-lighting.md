# Plan 4 — Lighting (zones, effects, brightness, idle)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** control asusd Aura on the keyboard, lightbar and logo: per-zone power states (boot/awake/sleep/shutdown), built-in effects with two colours/speed/direction, brightness with separate AC and battery levels, and an idle dim (`armoury kbd idle|resume`) that the Plan 5 shell service will drive from Quickshell's IdleMonitor.

**Architecture:** An `Aura` hardware trait over asusd's `xyz.ljones.Aura` object (found via the asusd ObjectManager; `/xyz/ljones/aura/19b6_3_3` here). Pure conversions/validation in `features/lighting.rs`. Brightness is read from sysfs (`asus::kbd_backlight`, observe-safe); effect and zone state come from asusd on request. AC/battery brightness is applied on power-source change in `tick`.

**Spec:** §4.4 (music lighting is Plan 9).

## Global Constraints

- Writes only in active mode; asusd calls through `with_retry`.
- Offer only modes/zones asusd reports (`SupportedBasicModes`, `SupportedPowerZones`).
- Idle dim never fights the user: `KbdResume` restores the level from before `KbdIdle`; `keep_on` disables idle dimming.

**Verified wire formats (asusd 6.4.0, captured on device):** `LedModeData (uu(yyy)(yyy)ss)` = mode, zone, colour1 rgb, colour2 rgb, speed `"Low"|"Med"|"High"`, direction `"Right"|"Left"|"Up"|"Down"`; `LedPower (a(ubbbb))` = struct holding an array of (zone, boot, awake, sleep, shutdown); `Brightness u` 0–3; `SupportedBasicModes au` = `0 1 2 3 4 5 6 7 8 10 11 12`; `SupportedPowerZones au` = `1 2 0`. Codes: modes Static 0, Breathe 1, RainbowCycle 2, RainbowWave 3, Star 4, Rain 5, Highlight 6, Laser 7, Ripple 8, Pulse 10, Comet 11, Flash 12; power zones Logo 0, Keyboard 1, Lightbar 2, Lid 3, RearGlow 4.

## Review Focus

1. Unsupported mode/zone requested → refused, nothing written (`unsupported_mode_refused`).
2. `KbdIdle` twice then `KbdResume` → restores the original level, not 0 (`idle_twice_restores_original`).
3. Power source flips AC→battery→AC → the matching stored brightness each time, no writes when unchanged (`brightness_follows_power_source`).
4. asusd has no Aura object (other models) → lighting requests fail with a clear message, no panic (`no_aura_device`).
5. Observe mode → writes refused; `light` read still works from sysfs.

---

### Task 1: Protocol
`crates/proto/src/lib.rs`: `AuraMode` enum (the 12 modes; `code()`/`from_code()`), `Speed {Low, Med, High}`, `Direction {Right, Left, Up, Down}`, `AuraZone {Logo, Keyboard, Lightbar, Lid, RearGlow}` (`code()`/`from_code()`), `AuraEffect { mode, colour1: [u8;3], colour2: [u8;3], speed, direction }`, `ZonePower { zone, boot, awake, sleep, shutdown }`, `LightingInfo { effect: Option<AuraEffect>, zones: Vec<ZonePower>, modes: Vec<AuraMode>, power_zones: Vec<AuraZone> }`; `Snapshot.lighting: LightingState { brightness: Option<u8>, on_ac: Option<bool> }`; requests `Lighting`, `SetBrightness { level: u8 }`, `SetEffect { effect: AuraEffect }`, `SetZonePower { zone: ZonePower }`, `KbdIdle`, `KbdResume`. Test wire formats + code round trips. Commit.

### Task 2: Conversions
`crates/armouryd/src/features/lighting.rs`: `type RawMode = (u32, u32, (u8,u8,u8), (u8,u8,u8), String, String)`, `type RawPower = (Vec<(u32, bool, bool, bool, bool)>,)`; `effect_from_raw`, `effect_to_raw`, `power_from_raw`, `set_zone(raw: &RawPower, z: &ZonePower) -> RawPower` (replace that zone's entry, keep others), `validate_effect(e, supported: &[AuraMode])`. Tests using the captured values (`0 0 166 0 0 0 0 0 "Med" "Right"`; power `3 1 true true false false 2 … 0 …`). Commit.

### Task 3: Aura hardware
`hw/mod.rs` trait `Aura: Send + Sync { async fn info(&self) -> Result<(RawMode, RawPower, Vec<u32>, Vec<u32>)>; async fn set_mode_data(&self, RawMode) -> Result<()>; async fn set_power(&self, RawPower) -> Result<()>; async fn set_brightness(&self, u32) -> Result<()>; }` + Arc forwarding; `hw/aura.rs` `AuraClient` (finds the first object implementing `xyz.ljones.Aura` with `DeviceType` 0 via `org.freedesktop.DBus.ObjectManager.GetManagedObjects` on `/`; uncached proxies; clear error "asusd has no Aura keyboard device"); `FakeAura` recording calls and holding state. Signature test: `<RawMode as Type>::SIGNATURE == "(uu(yyy)(yyy)ss)"`, `<RawPower as Type>::SIGNATURE == "(a(ubbbb))"`. Commit.

### Task 4: Readings
`sysfs::KBD_BRIGHTNESS = "sys/class/leds/asus::kbd_backlight/brightness"`; `Snapshot.lighting.brightness` from it; `on_ac` = any `power_supply/*` with `type == "Mains"` and `online == 1`. Test. Commit.

### Task 5: Daemon
`Daemon::new` gains `aura: Box<dyn Aura>` (last parameter). Config gains `lighting: LightingConfig { brightness_ac: Option<u8>, brightness_battery: Option<u8>, keep_on: bool }` (serde default). Handlers: `Lighting` → info from asusd converted; `SetBrightness{level}` → guard, level ≤ 3, `set_brightness`, store as the current source's level, save; `SetEffect` → guard, validate against supported modes, `set_mode_data`; `SetZonePower` → guard, zone supported, read power, `set_zone`, `set_power`; `KbdIdle` → if `!keep_on` and not already idle: remember current sysfs brightness, set 0; `KbdResume` → if idle: restore remembered level. `tick` (active): when `on_ac` changes and a level is stored for the new source → `set_brightness` (skip if equal to current). Tests: Review Focus 1–5 plus `set_effect_writes_mode_data`, `zone_power_keeps_other_zones`. Commit.

### Task 6: CLI
`armoury light` (brightness, AC/battery, effect, zones), `armoury light brightness <off|low|med|high|0-3>`, `armoury light effect <mode> [--color RRGGBB] [--color2 RRGGBB] [--speed low|med|high] [--direction right|left|up|down]`, `armoury light zone <keyboard|lightbar|logo|lid|rear> [--boot on|off] [--awake …] [--sleep …] [--shutdown …]` (unspecified states keep current values: CLI reads `Lighting` first), `armoury kbd idle|resume`. Tests for colour and mode parsing. Commit.

### Task 7: On-device
Takeover; `armoury light`; `light effect static --color ff0000`; `light brightness low` then `high`; `light zone logo --awake off` then `--awake on` (user confirms logo turns off/on); `kbd idle` (off) / `kbd resume` (back); handback (G-Helper restores its own lighting).
