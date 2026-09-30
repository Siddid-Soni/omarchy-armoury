# Plan 8 — NumberPad Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The illuminated NumberPad works under armouryd as it does in G-Helper today: hold the top-right icon to toggle; the cursor stops, keys are typed and the backlight is on while it's on. The user's defaults, each configurable: always starts at maximum brightness, does nothing while the touchpad is off, goes dark after 60 s without a touch.

**Architecture:**
- **Pure core** in `features/numpad/`:
  - `layout.rs`: key grid, hit-testing, i2c packet
  - `mt.rs`: multitouch slots → first-finger Down/Move/Up
  - `pad.rs`: the state machine, turning touches and clock ticks into actions
- **Async worker** (`worker.rs`): owns the touchpad reader, the virtual keyboard and the i2c backlight behind a small `NumpadIo` trait. It takes commands from the daemon over a channel and publishes its state to a shared cell.
- **Daemon side:** requests, config, snapshot, key action, and feeding "active?" / "touchpad on?" to the worker every tick.

**Tech Stack:** Rust (tokio, `evdev` 0.13 with the `tokio` feature, `i2cdev` 0.6; already added to `crates/armouryd/Cargo.toml` on this branch), QML (Quickshell/Omarchy `qs.Ui`), udev.

**Spec:** `docs/superpowers/specs/2026-09-30-numberpad-design.md` (extends `2026-09-29-omarchy-armoury-design.md` §4.6)

**Branch:** `plan-8-numpad` (from `main` at 52e00e1; carries the uncommitted `Cargo.toml`/`Cargo.lock` change adding evdev + i2cdev — commit it in Task 1).

## Global Constraints

- **Only in active mode.** In observe mode the worker keeps the pad off: no grab, no backlight writes, no keys. G-Helper's NumberPad owns it then.
- **Defaults** (`config.toml` `[numpad]`):
  - `start_brightness = 8` (1–8)
  - `allow_when_touchpad_off = false`
  - `idle_dim_secs = 60` (0 = never; max 3600)
  - `hold_ms = 1000` (300–3000)
- **Backlight packet**, sent to i2c address `0x15` (`0x38` for ASUF1416, ASUF1205, ASUF1204), with a raw `I2C_RDWR` write (no slave binding, since the HID driver owns the address):
  `[0x05,0x00,0x3d,0x03,0x06,0x00,0x07,0x00,0x0d,0x14,0x03,V,0xad]`
  - `V = 0x01` on, `0x00` off, `0x40+level` for levels 1–8.
- **g533 layout:**
  - rows `7 8 9 /`, `4 5 6 * ⌫`, `1 2 3 - ↵`, `0 0 . + ↵`
  - margins: top 200, right 200, left 200, bottom 80
  - right icon 1000 × 600, left icon 250 × 250
  - ASUE1403 uses g533
- **Keys:** keypad codes KP0–KP9 = 82,79,80,81,75,76,77,71,72,73; KPSLASH 98, KPASTERISK 55, KPMINUS 74, KPPLUS 78, KPDOT 83, KPENTER 96, BACKSPACE 14, NUMLOCK 69.
- **Behaviour:**
  - The worker never grabs the touchpad while the NumberPad is off.
  - The waking touch after the idle dim types nothing.
  - Only the first finger counts.
  - The NumberPad starts off after a restart.
- **No asusd or EC calls** anywhere in the NumberPad code.
- **Hardware tests** end with `armoury handback` (G-Helper running, asusd masked).

## Review Focus

1. **The grab must never outlive "on":**
   - device loss, observe mode, touchpad disabled and worker shutdown all ungrab (or drop the fd), so the cursor always comes back;
   - a toggle-off while a key is held releases the key first (no stuck key).
   - Test in Task 4.
2. **The touchpad is disabled while the NumberPad is on:** it turns off (ungrab, light off). Test in Task 3.
3. **A finger resting in the icon while the pad turns on:** the same held touch must not immediately toggle it off again. The hold fires once per touch. Test in Task 3.
4. **A second finger during a key press:** it must neither press a key nor release the first finger's key. Test in Task 2 (mt decoder).
5. **Config changes while on:**
   - a new `idle_dim_secs` applies from the next tick;
   - `allow_when_touchpad_off = false` with the touchpad off turns the pad off.
   - Test in Task 4.

---

## File Structure

- `crates/proto/src/lib.rs`:
  - `NumpadState`
  - `SystemState.numpad`
  - `KeyAction::ToggleNumpad`
  - `Request::SetNumpad` and `Request::SetNumpadConfig`
- `crates/armouryd/src/config.rs`: `NumpadConfig` (defaults, `validate`), `Config.numpad`.
- `crates/armouryd/src/features/numpad/mod.rs`: module root, re-exports.
- `crates/armouryd/src/features/numpad/layout.rs`: `Layout`, `G533`, `layout_for`, `Area`, `Hit`, `backlight_packet`, `i2c_address`, key constants.
- `crates/armouryd/src/features/numpad/mt.rs`: `MtDecoder` (raw slot events → `Touch`).
- `crates/armouryd/src/features/numpad/pad.rs`: `Pad` state machine (`Touch` + ticks → `Action`s).
- `crates/armouryd/src/features/numpad/worker.rs`:
  - `NumpadIo` trait, `Cmd`, `run_worker`
  - real IO: `EvdevIo` (touchpad + uinput + i2c)
  - `FakeIo` for tests
- `crates/armouryd/src/hw/hypr.rs` and `hw/fake.rs`: `Hypr::numlock()`.
- `crates/armouryd/src/ipc.rs`:
  - `with_numpad`
  - requests
  - snapshot field
  - Env feed in the tick
  - `ToggleNumpad` hotkey action
- `crates/armouryd/src/main.rs`: spawn the worker.
- `crates/armoury/src/main.rs`: `armoury numpad on|off`.
- `packaging/udev/70-omarchy-armoury.rules`: touchpad, i2c-dev and uinput uaccess lines.
- `tests/uninstall_test.sh`: rule check updated.
- `plugin/Widget.qml`: NumberPad tile.
- `plugin/InputPage.qml`: NumberPad section.

---

### Task 1: Protocol and config

**Files:** Modify `crates/proto/src/lib.rs`, `crates/armouryd/src/config.rs`, `crates/armouryd/src/state.rs` (SystemState initializer), `Cargo.lock`, `crates/armouryd/Cargo.toml` (already changed).

**Interfaces — Produces:**
- `armoury_proto::NumpadState { Unavailable, Off, On }`: serde snake_case, `Default = Unavailable`.
- `SystemState.numpad: NumpadState` (`#[serde(default)]`).
- `KeyAction::ToggleNumpad` (wire name `toggle_numpad`).
- `Request::SetNumpad { on: bool }`.
- `Request::SetNumpadConfig { start_brightness: Option<u8>, allow_when_touchpad_off: Option<bool>, idle_dim_secs: Option<u32>, hold_ms: Option<u32> }`, with every field `#[serde(default)]`.
- `config::NumpadConfig { start_brightness: u8, allow_when_touchpad_off: bool, idle_dim_secs: u32, hold_ms: u32 }` with `Default`, and `fn validate(&self) -> Result<(), String>`; plus `Config.numpad`.

- [ ] **Step 1: Failing tests.** Proto (append to `tests`):

```rust
    #[test]
    fn numpad_wire_format() {
        assert_eq!(serde_json::to_string(&NumpadState::On).unwrap(), "\"on\"");
        assert_eq!(NumpadState::default(), NumpadState::Unavailable);
        let r: Request = serde_json::from_str(r#"{"cmd":"set_numpad","on":true}"#).unwrap();
        assert_eq!(r, Request::SetNumpad { on: true });
        let r: Request = serde_json::from_str(r#"{"cmd":"set_numpad_config","idle_dim_secs":0}"#).unwrap();
        assert_eq!(r, Request::SetNumpadConfig { start_brightness: None, allow_when_touchpad_off: None, idle_dim_secs: Some(0), hold_ms: None });
        assert_eq!(serde_json::from_str::<KeyAction>("\"toggle_numpad\"").unwrap(), KeyAction::ToggleNumpad);
    }
```

Config (append to `config.rs` tests):

```rust
    #[test]
    fn numpad_defaults_and_validation() {
        let c = NumpadConfig::default();
        assert_eq!((c.start_brightness, c.allow_when_touchpad_off, c.idle_dim_secs, c.hold_ms), (8, false, 60, 1000));
        assert!(c.validate().is_ok());
        for bad in [NumpadConfig { start_brightness: 0, ..c }, NumpadConfig { start_brightness: 9, ..c },
                    NumpadConfig { hold_ms: 200, ..c }, NumpadConfig { hold_ms: 3001, ..c }, NumpadConfig { idle_dim_secs: 3601, ..c }] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        let (cfg, err) = { let d = tempfile::tempdir().unwrap(); let p = d.path().join("c.toml");
            std::fs::write(&p, "[numpad]\nidle_dim_secs = 0\n").unwrap(); Config::load(&p) };
        assert!(err.is_none());
        assert_eq!((cfg.numpad.idle_dim_secs, cfg.numpad.start_brightness), (0, 8), "missing keys take defaults");
    }
```

- [ ] **Step 2: Run** `cargo test --workspace`. Expected: compile errors (types missing).

- [ ] **Step 3: Implement.** Proto, next to `SystemState`:

```rust
/// Illuminated NumberPad on the touchpad (armouryd's, active mode only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumpadState {
    /// No supported touchpad, observe mode, or the worker gave up.
    #[default]
    Unavailable,
    Off,
    On,
}
```

- Add `#[serde(default)] pub numpad: NumpadState,` to `SystemState`, and `ToggleNumpad,` to `KeyAction`, with a doc comment ("Turns the NumberPad on or off.").
- Add to `Request`:

```rust
    /// NumberPad on/off (active mode; refused while the touchpad is off unless allowed).
    SetNumpad { on: bool },
    SetNumpadConfig {
        #[serde(default)] start_brightness: Option<u8>,
        #[serde(default)] allow_when_touchpad_off: Option<bool>,
        #[serde(default)] idle_dim_secs: Option<u32>,
        #[serde(default)] hold_ms: Option<u32>,
    },
```

Config:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NumpadConfig {
    /// Backlight level 1–8 used every time the NumberPad turns on.
    pub start_brightness: u8,
    /// Let the NumberPad work while Omarchy's touchpad toggle is off.
    pub allow_when_touchpad_off: bool,
    /// Backlight off after this long without a touch (0 = never).
    pub idle_dim_secs: u32,
    /// Top-right icon hold that toggles the NumberPad.
    pub hold_ms: u32,
}

impl Default for NumpadConfig {
    fn default() -> Self { Self { start_brightness: 8, allow_when_touchpad_off: false, idle_dim_secs: 60, hold_ms: 1000 } }
}

impl NumpadConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=8).contains(&self.start_brightness) { return Err("NumberPad brightness must be 1–8".into()); }
        if !(300..=3000).contains(&self.hold_ms) { return Err("hold time must be 300–3000 ms".into()); }
        if self.idle_dim_secs > 3600 { return Err("idle timeout must be 0–3600 s".into()); }
        Ok(())
    }
}
```

Add `pub numpad: NumpadConfig,` to `Config`. Add `numpad: NumpadState::Unavailable,` wherever `SystemState` is built literally (`state.rs`; compile errors point to each). The CLI and other `match`es on `KeyAction` get a `KeyAction::ToggleNumpad` arm. In `ipc.rs` `on_hotkey`, add `KeyAction::ToggleNumpad => {}` for now (Task 5 fills it in). The InputPage `actions` list is extended in Task 6. Add both new `Request` arms to the daemon's `handle` match as `=> Response::err("NumberPad not available")` for now (Task 5 replaces them).

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add Cargo.lock crates && git commit -m "feat(proto,config): NumberPad state, requests, config defaults; add evdev + i2cdev"
```

---

### Task 2: Layout, packet and multitouch decoder (pure)

**Files:**
- Create: `crates/armouryd/src/features/numpad/{mod.rs,layout.rs,mt.rs}`
- Modify: `crates/armouryd/src/features/mod.rs` (`pub mod numpad;`)

**Interfaces — Produces:**
- `layout::Area { minx: i32, maxx: i32, miny: i32, maxy: i32 }`
- `layout::Hit { Key(u16), RightIcon, LeftIcon, None }` (Copy, PartialEq)
- `layout::Layout` with `pub fn hit(&self, a: &Area, x: i32, y: i32) -> Hit`
- `layout::G533: Layout`, `layout::layout_for(touchpad_name: &str) -> Option<&'static Layout>`
- `layout::backlight_packet(v: u8) -> [u8; 13]`, `layout::i2c_address(touchpad_name: &str) -> u16`
- `layout::{LIGHT_ON, LIGHT_OFF}: u8`, `layout::level_byte(level: u8) -> u8`, `layout::KEY_NUMLOCK: u16`
- `mt::Raw { Slot(i32), TrackingId(i32), X(i32), Y(i32), Syn }` and `mt::Touch { Down { x: i32, y: i32 }, Move { x: i32, y: i32 }, Up }`
- `mt::MtDecoder::default()`, `fn feed(&mut self, r: Raw) -> Option<Touch>`: emits at `Syn` only, and only for the first finger.

- [ ] **Step 1: Failing tests** in `layout.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // this laptop's touchpad: X 0..4036, Y 0..2299
    const A: Area = Area { minx: 0, maxx: 4036, miny: 0, maxy: 2299 };
    // centre of cell (col,row) inside the g533 margins
    fn cell(col: i32, row: i32) -> (i32, i32) {
        let (w, h) = ((4036 - 200 - 200) as f32 / 5.0, (2299 - 200 - 80) as f32 / 4.0);
        ((200.0 + w * (col as f32 + 0.5)) as i32, (200.0 + h * (row as f32 + 0.5)) as i32)
    }

    #[test]
    fn every_g533_cell() {
        let want = [[71, 72, 73, 98, 0], [75, 76, 77, 55, 14], [79, 80, 81, 74, 96], [82, 82, 83, 78, 96]];
        for (row, keys) in want.iter().enumerate() {
            for (col, &k) in keys.iter().enumerate() {
                let (x, y) = cell(col as i32, row as i32);
                let got = G533.hit(&A, x, y);
                let expect = if k == 0 { Hit::None } else { Hit::Key(k) };
                if row == 0 && col == 4 { assert_eq!(got, Hit::RightIcon, "row 0 col 4 is the icon"); } else { assert_eq!(got, expect, "row {row} col {col}"); }
            }
        }
    }

    #[test]
    fn margins_and_icons() {
        assert_eq!(G533.hit(&A, 100, 1200), Hit::None, "left margin");
        assert_eq!(G533.hit(&A, 2000, 2250), Hit::None, "bottom margin");
        assert_eq!(G533.hit(&A, 4000, 50), Hit::RightIcon);
        assert_eq!(G533.hit(&A, 100, 100), Hit::LeftIcon);
        assert!(layout_for("ASUE1403:00 04F3:319A Touchpad").is_some());
        assert!(layout_for("SYNA1234 Touchpad").is_none());
    }

    #[test]
    fn packet_and_address() {
        assert_eq!(backlight_packet(LIGHT_ON), [0x05, 0x00, 0x3d, 0x03, 0x06, 0x00, 0x07, 0x00, 0x0d, 0x14, 0x03, 0x01, 0xad]);
        assert_eq!(backlight_packet(level_byte(8))[11], 0x48);
        assert_eq!(level_byte(1), 0x41);
        assert_eq!(i2c_address("ASUE1403:00 04F3:319A Touchpad"), 0x15);
        assert_eq!(i2c_address("ASUF1416:00 2808:0108 Touchpad"), 0x38);
    }
}
```

In `mt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use Raw::*;
    fn run(d: &mut MtDecoder, rs: &[Raw]) -> Vec<Touch> { rs.iter().filter_map(|r| d.feed(*r)).collect() }

    #[test]
    fn one_finger_down_move_up() {
        let mut d = MtDecoder::default();
        assert_eq!(run(&mut d, &[Slot(0), TrackingId(7), X(100), Y(200), Syn]), [Touch::Down { x: 100, y: 200 }]);
        assert_eq!(run(&mut d, &[X(150), Syn]), [Touch::Move { x: 150, y: 200 }]);
        assert_eq!(run(&mut d, &[TrackingId(-1), Syn]), [Touch::Up]);
    }

    #[test]
    fn second_finger_is_ignored() {
        let mut d = MtDecoder::default();
        run(&mut d, &[Slot(0), TrackingId(1), X(100), Y(100), Syn]);
        // second finger lands and lifts: no events for it, first finger unaffected
        assert!(run(&mut d, &[Slot(1), TrackingId(2), X(900), Y(900), Syn]).is_empty());
        assert!(run(&mut d, &[Slot(1), TrackingId(-1), Syn]).is_empty());
        assert_eq!(run(&mut d, &[Slot(0), TrackingId(-1), Syn]), [Touch::Up]);
        // after the first lifts, a new finger becomes the first
        assert_eq!(run(&mut d, &[Slot(1), TrackingId(3), X(5), Y(6), Syn]), [Touch::Down { x: 5, y: 6 }]);
    }
}
```

- [ ] **Step 2: Run** `cargo test -p armouryd --lib numpad`. Expected: compile errors.

- [ ] **Step 3: Implement.** `features/numpad/mod.rs`:

```rust
//! Illuminated NumberPad: pure layout / touch logic plus the worker that owns the devices.
pub mod layout;
pub mod mt;
```

`layout.rs`:

```rust
//! NumberPad key grids (per model) and the touchpad backlight packet
//! (from asus-numberpad-driver; verified against G-Helper on the G533ZW).

pub const LIGHT_ON: u8 = 0x01;
pub const LIGHT_OFF: u8 = 0x00;
pub const KEY_NUMLOCK: u16 = 69;

/// Brightness level 1–8 → packet value.
pub fn level_byte(level: u8) -> u8 { 0x40 + level.clamp(1, 8) }

pub fn backlight_packet(v: u8) -> [u8; 13] {
    [0x05, 0x00, 0x3d, 0x03, 0x06, 0x00, 0x07, 0x00, 0x0d, 0x14, 0x03, v, 0xad]
}

pub fn i2c_address(touchpad_name: &str) -> u16 {
    if ["ASUF1416", "ASUF1205", "ASUF1204"].iter().any(|p| touchpad_name.starts_with(p)) { 0x38 } else { 0x15 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area { pub minx: i32, pub maxx: i32, pub miny: i32, pub maxy: i32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit { Key(u16), RightIcon, LeftIcon, None }

pub struct Layout {
    pub name: &'static str,
    /// Touchpad name prefixes this layout is for.
    pub touchpads: &'static [&'static str],
    /// Rows of evdev key codes; shorter rows leave dead cells on the right.
    pub keys: &'static [&'static [u16]],
    pub top: i32, pub right: i32, pub left: i32, pub bottom: i32,
    pub right_icon: (i32, i32),
    pub left_icon: (i32, i32),
}

impl Layout {
    pub fn hit(&self, a: &Area, x: i32, y: i32) -> Hit {
        if x >= a.maxx - self.right_icon.0 && y <= a.miny + self.right_icon.1 { return Hit::RightIcon; }
        if x <= a.minx + self.left_icon.0 && y <= a.miny + self.left_icon.1 { return Hit::LeftIcon; }
        let (x0, x1, y0, y1) = (a.minx + self.left, a.maxx - self.right, a.miny + self.top, a.maxy - self.bottom);
        if x < x0 || x >= x1 || y < y0 || y >= y1 { return Hit::None; }
        let cols = self.keys.iter().map(|r| r.len()).max().unwrap_or(1) as i32;
        let rows = self.keys.len() as i32;
        let col = ((x - x0) * cols / (x1 - x0)) as usize;
        let row = ((y - y0) * rows / (y1 - y0)) as usize;
        self.keys.get(row).and_then(|r| r.get(col)).map_or(Hit::None, |&k| Hit::Key(k))
    }
}

/// ROG Strix G533 (and G614JVR): 5 columns, NumLock icon top right.
pub const G533: Layout = Layout {
    name: "g533",
    touchpads: &["ASUE1403"],
    keys: &[&[71, 72, 73, 98], &[75, 76, 77, 55, 14], &[79, 80, 81, 74, 96], &[82, 82, 83, 78, 96]],
    top: 200, right: 200, left: 200, bottom: 80,
    right_icon: (1000, 600),
    left_icon: (250, 250),
};

const LAYOUTS: &[&Layout] = &[&G533];

pub fn layout_for(touchpad_name: &str) -> Option<&'static Layout> {
    LAYOUTS.iter().copied().find(|l| l.touchpads.iter().any(|p| touchpad_name.starts_with(p)))
}
```

`mt.rs`:

```rust
//! Multitouch protocol B → the first finger's Down / Move / Up (other fingers ignored).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw { Slot(i32), TrackingId(i32), X(i32), Y(i32), Syn }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch { Down { x: i32, y: i32 }, Move { x: i32, y: i32 }, Up }

pub struct MtDecoder {
    slot: usize,
    /// (tracking id, x, y) per slot; tracking id < 0 = empty.
    slots: [(i32, i32, i32); 10],
    /// The slot holding the first finger, and whether its Down was already sent.
    first: Option<usize>,
    down_sent: bool,
    last: (i32, i32),
}

impl Default for MtDecoder {
    /// Every slot starts empty (tracking id -1).
    fn default() -> Self { Self { slot: 0, slots: [(-1, 0, 0); 10], first: None, down_sent: false, last: (0, 0) } }
}

impl MtDecoder {
    pub fn feed(&mut self, r: Raw) -> Option<Touch> {
        match r {
            Raw::Slot(s) => { self.slot = (s.max(0) as usize).min(9); None }
            Raw::TrackingId(id) => {
                self.slots[self.slot].0 = id;
                if id >= 0 && self.first.is_none() { self.first = Some(self.slot); self.down_sent = false; }
                None
            }
            Raw::X(x) => { self.slots[self.slot].1 = x; None }
            Raw::Y(y) => { self.slots[self.slot].2 = y; None }
            Raw::Syn => {
                let f = self.first?;
                let (id, x, y) = self.slots[f];
                if id < 0 {
                    self.first = None;
                    return self.down_sent.then_some(Touch::Up);
                }
                if !self.down_sent { self.down_sent = true; self.last = (x, y); return Some(Touch::Down { x, y }); }
                if (x, y) != self.last { self.last = (x, y); return Some(Touch::Move { x, y }); }
                None
            }
        }
    }
}
```

- [ ] **Step 4: Run** `cargo test -p armouryd --lib numpad`. Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates && git commit -m "feat(numpad): g533 layout, hit-testing, backlight packet, multitouch first-finger decoder"
```

---

### Task 3: Pad state machine (pure)

**Files:**
- Create: `crates/armouryd/src/features/numpad/pad.rs`
- Modify: `mod.rs` (`pub mod pad;`)

**Interfaces — Consumes:** `layout::{Hit, LIGHT_OFF, level_byte, KEY_NUMLOCK}`, `mt::Touch`.
**Produces:**
- `pad::Action { Grab(bool), Light(u8) /* packet value */, Key { code: u16, down: bool }, EnsureNumlock }`
- `pad::Settings { hold: Duration, idle: Option<Duration>, start_level: u8 }`
- `pad::Pad::new() -> Pad`, `fn is_on(&self) -> bool`, `fn set_allowed(&mut self, allowed: bool) -> Vec<Action>`
- `fn set_on(&mut self, on: bool, s: &Settings, now: Instant) -> Vec<Action>`
- `fn touch(&mut self, t: Touch, hit: Hit, s: &Settings, now: Instant) -> Vec<Action>`
- `fn tick(&mut self, s: &Settings, now: Instant) -> Vec<Action>`
- All times are `std::time::Instant`, passed in, so tests control the clock.

- [ ] **Step 1: Failing tests** in `pad.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::numpad::layout::{Hit, LIGHT_OFF, level_byte};
    use crate::features::numpad::mt::Touch;
    use std::time::{Duration, Instant};
    fn s() -> Settings { Settings { hold: Duration::from_millis(1000), idle: Some(Duration::from_secs(60)), start_level: 8 } }
    fn ms(t0: Instant, n: u64) -> Instant { t0 + Duration::from_millis(n) }
    fn hold_icon(p: &mut Pad, t0: Instant) -> Vec<Action> {
        let mut a = p.touch(Touch::Down { x: 4000, y: 50 }, Hit::RightIcon, &s(), t0);
        a.extend(p.tick(&s(), ms(t0, 1001)));
        a
    }

    #[test]
    fn hold_turns_on_at_start_brightness_with_grab_and_numlock() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        let a = hold_icon(&mut p, t0);
        assert_eq!(a, [Action::Grab(true), Action::Light(level_byte(8)), Action::EnsureNumlock]);
        assert!(p.is_on());
    }

    #[test]
    fn short_touch_on_icon_does_nothing() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        p.touch(Touch::Down { x: 4000, y: 50 }, Hit::RightIcon, &s(), t0);
        assert!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 300)).is_empty());
        assert!(p.tick(&s(), ms(t0, 2000)).is_empty());
        assert!(!p.is_on());
    }

    #[test]
    fn hold_fires_once_per_touch() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        assert!(p.tick(&s(), ms(t0, 2500)).is_empty(), "still the same touch: no toggle back off");
        assert!(p.is_on());
    }

    #[test]
    fn keys_press_on_down_release_on_up_only_while_on() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        assert!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), t0).is_empty(), "off: no keys");
        p.touch(Touch::Up, Hit::None, &s(), t0);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        assert_eq!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 1200)), [Action::Key { code: 72, down: true }]);
        assert!(p.touch(Touch::Move { x: 900, y: 900 }, Hit::Key(80), &s(), ms(t0, 1250)).is_empty(), "moving doesn't retype");
        assert_eq!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1300)), [Action::Key { code: 72, down: false }]);
    }

    #[test]
    fn left_icon_tap_steps_brightness() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        p.touch(Touch::Down { x: 10, y: 10 }, Hit::LeftIcon, &s(), ms(t0, 1200));
        assert_eq!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1300)), [Action::Light(level_byte(1))], "8 wraps to 1");
    }

    #[test]
    fn idle_goes_dark_and_waking_touch_types_nothing() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        assert_eq!(p.tick(&s(), ms(t0, 1100 + 60_000)), [Action::Light(LIGHT_OFF)]);
        assert!(p.is_on(), "still on (grabbed), just dark");
        assert_eq!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 70_000)), [Action::Light(level_byte(8))]);
        assert!(p.touch(Touch::Up, Hit::None, &s(), ms(t0, 70_100)).is_empty(), "no key up for a key never pressed");
        assert_eq!(p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 70_200)), [Action::Key { code: 72, down: true }]);
    }

    #[test]
    fn idle_never_when_disabled() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        let never = Settings { idle: None, ..s() };
        assert!(p.tick(&never, ms(t0, 10_000_000)).is_empty());
    }

    #[test]
    fn hold_ignored_when_not_allowed_and_disallow_turns_off() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        assert!(hold_icon(&mut p, t0).is_empty(), "touchpad off: the hold does nothing");
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        p.set_allowed(true);
        hold_icon(&mut p, ms(t0, 2000));
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 3100));
        p.touch(Touch::Down { x: 1, y: 1 }, Hit::Key(72), &s(), ms(t0, 3200)); // key held
        assert_eq!(p.set_allowed(false), [Action::Key { code: 72, down: false }, Action::Light(LIGHT_OFF), Action::Grab(false)]);
        assert!(!p.is_on());
    }

    #[test]
    fn hold_again_turns_off_releasing_nothing_stuck() {
        let (mut p, t0) = (Pad::new(), Instant::now());
        p.set_allowed(true);
        hold_icon(&mut p, t0);
        p.touch(Touch::Up, Hit::None, &s(), ms(t0, 1100));
        let a = hold_icon(&mut p, ms(t0, 2000));
        assert_eq!(a, [Action::Light(LIGHT_OFF), Action::Grab(false)]);
    }
}
```

- [ ] **Step 2: Run** `cargo test -p armouryd --lib pad::`. Expected: compile errors.

- [ ] **Step 3: Implement** `pad.rs`:

```rust
//! NumberPad behaviour as a pure state machine: touches and clock ticks in, device actions out.
use super::layout::{level_byte, Hit, LIGHT_OFF};
use super::mt::Touch;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action { Grab(bool), Light(u8), Key { code: u16, down: bool }, EnsureNumlock }

#[derive(Debug, Clone, Copy)]
pub struct Settings { pub hold: Duration, pub idle: Option<Duration>, pub start_level: u8 }

pub struct Pad {
    on: bool,
    allowed: bool,
    level: u8,
    lit: bool,
    /// Key held by the current touch.
    pressed: Option<u16>,
    /// The current touch started in the right icon at this time (and hasn't left it).
    hold_since: Option<Instant>,
    hold_fired: bool,
    left_tap: bool,
    /// The current touch only woke the backlight: it types nothing.
    waking: bool,
    finger_down: bool,
    last_touch: Instant,
}

impl Pad {
    pub fn new() -> Self {
        Self { on: false, allowed: false, level: 8, lit: false, pressed: None, hold_since: None, hold_fired: false,
               left_tap: false, waking: false, finger_down: false, last_touch: Instant::now() }
    }

    pub fn is_on(&self) -> bool { self.on }

    /// Touchpad enabled (or allowed while off) and armouryd active; turning false while on turns it off.
    pub fn set_allowed(&mut self, allowed: bool) -> Vec<Action> {
        self.allowed = allowed;
        if !allowed && self.on { self.turn_off() } else { Vec::new() }
    }

    pub fn set_on(&mut self, on: bool, s: &Settings, now: Instant) -> Vec<Action> {
        match (on, self.on) {
            (true, false) if self.allowed => self.turn_on(s, now),
            (false, true) => self.turn_off(),
            _ => Vec::new(),
        }
    }

    fn turn_on(&mut self, s: &Settings, now: Instant) -> Vec<Action> {
        self.on = true;
        self.lit = true;
        self.level = s.start_level.clamp(1, 8);
        self.last_touch = now;
        vec![Action::Grab(true), Action::Light(level_byte(self.level)), Action::EnsureNumlock]
    }

    fn turn_off(&mut self) -> Vec<Action> {
        let mut a = Vec::new();
        if let Some(code) = self.pressed.take() { a.push(Action::Key { code, down: false }); }
        self.on = false;
        self.lit = false;
        a.push(Action::Light(LIGHT_OFF));
        a.push(Action::Grab(false));
        a
    }

    pub fn touch(&mut self, t: Touch, hit: Hit, s: &Settings, now: Instant) -> Vec<Action> {
        let mut a = Vec::new();
        match t {
            Touch::Down { .. } => {
                self.finger_down = true;
                self.last_touch = now;
                self.hold_fired = false;
                self.hold_since = (hit == Hit::RightIcon).then_some(now);
                if self.on && !self.lit {
                    self.lit = true;
                    self.waking = true;
                    a.push(Action::Light(level_byte(self.level)));
                    return a;
                }
                if !self.on { return a; }
                match hit {
                    Hit::Key(code) => { self.pressed = Some(code); a.push(Action::Key { code, down: true }); }
                    Hit::LeftIcon => self.left_tap = true,
                    _ => {}
                }
            }
            Touch::Move { .. } => {
                self.last_touch = now;
                if hit != Hit::RightIcon { self.hold_since = None; }
                if hit != Hit::LeftIcon { self.left_tap = false; }
            }
            Touch::Up => {
                self.finger_down = false;
                self.last_touch = now;
                self.hold_since = None;
                self.waking = false;
                if let Some(code) = self.pressed.take() { a.push(Action::Key { code, down: false }); }
                if std::mem::take(&mut self.left_tap) && self.on {
                    self.level = self.level % 8 + 1;
                    a.push(Action::Light(level_byte(self.level)));
                }
            }
        }
        let _ = s;
        a
    }

    pub fn tick(&mut self, s: &Settings, now: Instant) -> Vec<Action> {
        if let Some(t) = self.hold_since {
            if !self.hold_fired && now.duration_since(t) >= s.hold {
                self.hold_fired = true;
                self.hold_since = None;
                return if self.on { self.turn_off() } else if self.allowed { self.turn_on(s, now) } else { Vec::new() };
            }
        }
        if let Some(idle) = s.idle {
            if self.on && self.lit && !self.finger_down && now.duration_since(self.last_touch) >= idle {
                self.lit = false;
                return vec![Action::Light(LIGHT_OFF)];
            }
        }
        Vec::new()
    }
}
```

The `Settings` parameter of `touch` is unused, but it's kept for a symmetric API; `let _ = s;` avoids the warning.

Check `hold_fires_once_per_touch`: after firing, `hold_since = None` and `hold_fired = true`, so a later tick in the same touch does nothing.

Check `keys_press…` step 1: while off, a touch on a key yields nothing; `finger_down` resets on Up.

Check `idle_goes_dark…`: the waking Down returns early without pressing, so the next Up has no `pressed` and yields nothing.

- [ ] **Step 4: Run** `cargo test -p armouryd --lib pad::`. Expected: PASS. Fix the implementation, not the tests, if one fails. The tests encode the spec.

- [ ] **Step 5: Commit.**

```bash
git add crates && git commit -m "feat(numpad): pad state machine — hold toggle, keys, brightness step, idle dim, wake touch, touchpad-off rule"
```

---

### Task 4: Worker, device IO, and Hypr NumLock

**Files:**
- Create: `crates/armouryd/src/features/numpad/worker.rs`
- Modify: `mod.rs` (`pub mod worker;`), `crates/armouryd/src/hw/hypr.rs` (`numlock`), `crates/armouryd/src/hw/fake.rs` (`FakeHypr.numlock`)

**Interfaces — Consumes:** `Pad`, `Settings`, `Action`, `MtDecoder`, `Raw`, `Touch`, `Layout`, `Area`, `backlight_packet`, `KEY_NUMLOCK`, `config::NumpadConfig`, `hw::hypr::Hypr`.

**Produces:**
- `Hypr::numlock(&self) -> anyhow::Result<bool>`: true if any keyboard in `hyprctl devices -j` reports `numLock: true`.
- `worker::Cmd { Env { active: bool, touchpad_enabled: bool }, Set(bool), Config(NumpadConfig) }`
- `worker::NumpadIo` (async trait):
  - `async fn next(&mut self) -> std::io::Result<Raw>`
  - `fn area(&self) -> Area`
  - `fn layout(&self) -> &'static Layout`
  - `fn grab(&mut self, on: bool) -> std::io::Result<()>`
  - `fn light(&mut self, v: u8) -> std::io::Result<()>`
  - `fn key(&mut self, code: u16, down: bool) -> std::io::Result<()>`
- `worker::open_real() -> std::io::Result<EvdevIo>`: finds the touchpad by name, opens it, creates the uinput "armoury numberpad" device and the i2c bus.
- `worker::run_worker(open: impl Fn() -> io::Result<Box<dyn NumpadIo>>, cmds: mpsc::Receiver<Cmd>, state: Arc<std::sync::Mutex<NumpadState>>, hypr: Arc<dyn Hypr>, cfg: NumpadConfig)`: runs forever.

- [ ] **Step 1: Failing tests** in `worker.rs`. Use a fake IO fed through a channel, with tokio's paused clock:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::numpad::layout::{G533, LIGHT_OFF, level_byte};
    use crate::hw::fake::FakeHypr;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    /// Fake touchpad: raw events come from a channel; actions are logged.
    struct FakeIo { rx: mpsc::UnboundedReceiver<std::io::Result<Raw>>, log: Arc<Mutex<Vec<String>>> }
    #[async_trait::async_trait]
    impl NumpadIo for FakeIo {
        async fn next(&mut self) -> std::io::Result<Raw> {
            match self.rx.recv().await { Some(r) => r, None => std::future::pending().await }
        }
        fn area(&self) -> Area { Area { minx: 0, maxx: 4036, miny: 0, maxy: 2299 } }
        fn layout(&self) -> &'static Layout { &G533 }
        fn grab(&mut self, on: bool) -> std::io::Result<()> { self.log.lock().unwrap().push(format!("grab {on}")); Ok(()) }
        fn light(&mut self, v: u8) -> std::io::Result<()> { self.log.lock().unwrap().push(format!("light {v:#04x}")); Ok(()) }
        fn key(&mut self, code: u16, down: bool) -> std::io::Result<()> { self.log.lock().unwrap().push(format!("key {code} {down}")); Ok(()) }
    }

    struct Rig { tx: mpsc::UnboundedSender<std::io::Result<Raw>>, cmds: mpsc::Sender<Cmd>, log: Arc<Mutex<Vec<String>>>, state: Arc<Mutex<NumpadState>>, hypr: Arc<FakeHypr> }

    fn start() -> Rig {
        let (tx, rx) = mpsc::unbounded_channel();
        let rx = Arc::new(Mutex::new(Some(rx)));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (ctx, crx) = mpsc::channel(8);
        let state = Arc::new(Mutex::new(NumpadState::Unavailable));
        let hypr = Arc::new(FakeHypr::default());
        let l = log.clone();
        let open = move || -> std::io::Result<Box<dyn NumpadIo>> {
            match rx.lock().unwrap().take() {
                Some(rx) => Ok(Box::new(FakeIo { rx, log: l.clone() })),
                None => Err(std::io::Error::other("no touchpad")),
            }
        };
        tokio::spawn(run_worker(open, crx, state.clone(), hypr.clone(), NumpadConfig::default()));
        Rig { tx, cmds: ctx, log, state, hypr }
    }
    fn touch(r: &Rig, x: i32, y: i32) { for e in [Raw::Slot(0), Raw::TrackingId(1), Raw::X(x), Raw::Y(y), Raw::Syn] { r.tx.send(Ok(e)).unwrap(); } }
    fn lift(r: &Rig) { for e in [Raw::TrackingId(-1), Raw::Syn] { r.tx.send(Ok(e)).unwrap(); } }
    async fn settle() { for _ in 0..20 { tokio::task::yield_now().await; } }
    fn log(r: &Rig) -> Vec<String> { r.log.lock().unwrap().clone() }

    #[tokio::test(start_paused = true)]
    async fn hold_on_type_hold_off() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        *r.hypr.numlock.lock().unwrap() = false;
        settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Off);
        touch(&r, 4000, 50); settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        lift(&r); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::On);
        touch(&r, 1000, 800); lift(&r); settle().await; // "5"
        let l = log(&r);
        assert_eq!(&l[..3], ["grab true", &format!("light {:#04x}", level_byte(8)), "key 69 true"], "{l:?}");
        assert!(l.contains(&"key 76 true".to_string()) && l.contains(&"key 76 false".to_string()), "{l:?}");
        touch(&r, 4000, 50); settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        lift(&r); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Off);
        assert_eq!(&log(&r)[log(&r).len() - 2..], [format!("light {LIGHT_OFF:#04x}"), "grab false".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn observe_mode_turns_it_off_and_ignores_holds() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::On);
        r.cmds.send(Cmd::Env { active: false, touchpad_enabled: true }).await.unwrap(); settle().await;
        assert!(log(&r).ends_with(&["grab false".to_string()]), "{:?}", log(&r));
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Unavailable);
        touch(&r, 4000, 50); settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await; settle().await;
        assert!(!log(&r).iter().rev().take(1).any(|l| l == "grab true"), "no hold in observe mode");
    }

    #[tokio::test(start_paused = true)]
    async fn device_loss_releases_everything() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        r.tx.send(Err(std::io::Error::other("gone"))).unwrap(); settle().await;
        assert!(log(&r).ends_with(&[format!("light {LIGHT_OFF:#04x}"), "grab false".to_string()]), "{:?}", log(&r));
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Unavailable);
    }

    #[tokio::test(start_paused = true)]
    async fn config_change_applies_while_on() {
        let r = start();
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: true }).await.unwrap();
        r.cmds.send(Cmd::Set(true)).await.unwrap(); settle().await;
        r.cmds.send(Cmd::Config(NumpadConfig { idle_dim_secs: 5, ..NumpadConfig::default() })).await.unwrap(); settle().await;
        tokio::time::sleep(std::time::Duration::from_secs(6)).await; settle().await;
        assert!(log(&r).ends_with(&[format!("light {LIGHT_OFF:#04x}")]), "new idle timeout used: {:?}", log(&r));
        r.cmds.send(Cmd::Env { active: true, touchpad_enabled: false }).await.unwrap(); settle().await;
        assert_eq!(*r.state.lock().unwrap(), NumpadState::Off, "touchpad off → NumberPad off");
    }
}
```

In `hw/hypr.rs` tests:

```rust
    #[test]
    fn numlock_from_devices_json() {
        assert!(parse_numlock(r#"{"keyboards":[{"name":"a","numLock":false},{"name":"b","numLock":true}]}"#).unwrap());
        assert!(!parse_numlock(r#"{"keyboards":[{"name":"a","numLock":false}]}"#).unwrap());
    }
```

- [ ] **Step 2: Run** `cargo test -p armouryd --lib worker:: numlock`. Expected: compile errors.

- [ ] **Step 3: Implement.**

**Hypr NumLock:** in `hw/hypr.rs`:
- Add `async fn numlock(&self) -> anyhow::Result<bool>;` to the trait, and the forwarding line to the `Arc` impl.
- Add the parser and the real implementation:

```rust
pub fn parse_numlock(json: &str) -> anyhow::Result<bool> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    Ok(v["keyboards"].as_array().into_iter().flatten().any(|k| k["numLock"].as_bool() == Some(true)))
}
```

```rust
    async fn numlock(&self) -> anyhow::Result<bool> { parse_numlock(&hyprctl(&["devices", "-j"]).await?) }
```

**FakeHypr:** in `hw/fake.rs`, add a `pub numlock: Mutex<bool>,` field (default `true` in its `Default`) and:

```rust
    async fn numlock(&self) -> anyhow::Result<bool> { Ok(*self.numlock.lock().unwrap()) }
```

`worker.rs`:

```rust
//! Owns the NumberPad devices: reads the touchpad, drives the Pad state machine, and
//! applies its actions (grab, backlight, virtual keys). Active mode only; any device error
//! turns everything off (grab released with the fd) and the touchpad is reopened later.
use super::layout::{backlight_packet, layout_for, Area, Hit, Layout, KEY_NUMLOCK, LIGHT_OFF};
use super::mt::{MtDecoder, Raw, Touch};
use super::pad::{Action, Pad, Settings};
use crate::config::NumpadConfig;
use crate::hw::hypr::Hypr;
use armoury_proto::NumpadState;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

pub enum Cmd { Env { active: bool, touchpad_enabled: bool }, Set(bool), Config(NumpadConfig) }

#[async_trait::async_trait]
pub trait NumpadIo: Send {
    async fn next(&mut self) -> std::io::Result<Raw>;
    fn area(&self) -> Area;
    fn layout(&self) -> &'static Layout;
    fn grab(&mut self, on: bool) -> std::io::Result<()>;
    fn light(&mut self, v: u8) -> std::io::Result<()>;
    fn key(&mut self, code: u16, down: bool) -> std::io::Result<()>;
}

fn settings(c: &NumpadConfig) -> Settings {
    Settings {
        hold: Duration::from_millis(c.hold_ms as u64),
        idle: (c.idle_dim_secs > 0).then(|| Duration::from_secs(c.idle_dim_secs as u64)),
        start_level: c.start_brightness,
    }
}

const TICK: Duration = Duration::from_millis(100);
/// More device failures than this within a minute: stop (reported as unavailable).
const MAX_FAILURES_PER_MIN: usize = 3;

pub async fn run_worker(
    open: impl Fn() -> std::io::Result<Box<dyn NumpadIo>> + Send + 'static,
    mut cmds: mpsc::Receiver<Cmd>,
    state: Arc<Mutex<NumpadState>>,
    hypr: Arc<dyn Hypr>,
    mut cfg: NumpadConfig,
) {
    let (mut active, mut tp_on) = (false, true);
    let mut failures: Vec<tokio::time::Instant> = Vec::new();
    let mut backoff = 0u32;
    loop {
        // wait until armouryd is active before touching the device
        while !active {
            match cmds.recv().await {
                Some(Cmd::Env { active: a, touchpad_enabled: t }) => { active = a; tp_on = t; }
                Some(Cmd::Config(c)) => cfg = c,
                Some(Cmd::Set(_)) => {}
                None => return,
            }
        }
        let mut io = match open() {
            Ok(io) => io,
            Err(e) => {
                eprintln!("armouryd: NumberPad: {e}");
                *state.lock().unwrap() = NumpadState::Unavailable;
                tokio::time::sleep(crate::features::keys::backoff(backoff)).await;
                backoff += 1;
                continue;
            }
        };
        backoff = 0;
        *state.lock().unwrap() = NumpadState::Off;
        let mut pad = Pad::new();
        let mut mt = MtDecoder::default();
        let area = io.area();
        let layout = io.layout();
        apply(&mut *io, &*hypr, pad.set_allowed(tp_on || cfg.allow_when_touchpad_off)).await;
        let mut tick = tokio::time::interval(TICK);
        let result: std::io::Result<()> = loop {
            let actions = tokio::select! {
                ev = io.next() => match ev {
                    Err(e) => break Err(e),
                    Ok(raw) => match mt.feed(raw) {
                        Some(t) => {
                            let (x, y) = match t { Touch::Down { x, y } | Touch::Move { x, y } => (x, y), Touch::Up => (0, 0) };
                            let hit = if matches!(t, Touch::Up) { Hit::None } else { layout.hit(&area, x, y) };
                            pad.touch(t, hit, &settings(&cfg), std::time::Instant::now())
                        }
                        None => Vec::new(),
                    },
                },
                _ = tick.tick() => pad.tick(&settings(&cfg), std::time::Instant::now()),
                cmd = cmds.recv() => match cmd {
                    None => break Ok(()),
                    Some(Cmd::Env { active: a, touchpad_enabled: t }) => {
                        active = a; tp_on = t;
                        if !active { break Ok(()); }
                        pad.set_allowed(tp_on || cfg.allow_when_touchpad_off)
                    }
                    Some(Cmd::Config(c)) => { cfg = c; pad.set_allowed(tp_on || cfg.allow_when_touchpad_off) }
                    Some(Cmd::Set(on)) => pad.set_on(on, &settings(&cfg), std::time::Instant::now()),
                },
            };
            if let Err(e) = apply(&mut *io, &*hypr, actions).await { break Err(e); }
            *state.lock().unwrap() = if pad.is_on() { NumpadState::On } else { NumpadState::Off };
        };
        // leaving: everything off; dropping `io` closes the fd, which also ends any grab
        let _ = apply(&mut *io, &*hypr, pad.set_allowed(false)).await;
        drop(io);
        *state.lock().unwrap() = NumpadState::Unavailable;
        if let Err(e) = result {
            eprintln!("armouryd: NumberPad device: {e}");
            let now = tokio::time::Instant::now();
            failures.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
            failures.push(now);
            if failures.len() > MAX_FAILURES_PER_MIN {
                eprintln!("armouryd: NumberPad stopped after repeated device failures");
                return;
            }
            tokio::time::sleep(crate::features::keys::backoff(0)).await;
        }
    }
}

/// Backlight failures are logged, not fatal (the NumberPad works unlit); grab / key failures end the session.
async fn apply(io: &mut dyn NumpadIo, hypr: &dyn Hypr, actions: Vec<Action>) -> std::io::Result<()> {
    for a in actions {
        match a {
            Action::Grab(on) => io.grab(on)?,
            Action::Light(v) => if let Err(e) = io.light(v) { eprintln!("armouryd: NumberPad backlight: {e}"); },
            Action::Key { code, down } => io.key(code, down)?,
            Action::EnsureNumlock => {
                if hypr.numlock().await.ok() == Some(false) {
                    io.key(KEY_NUMLOCK, true)?;
                    io.key(KEY_NUMLOCK, false)?;
                }
            }
        }
    }
    Ok(())
}
```

Leaving a session (observe mode, device loss, channel closed) always runs `set_allowed(false)`, which yields Light(OFF) then Grab(false) when the pad was on, so the pad is never left lit or grabbed for G-Helper. The state then reads `Unavailable` until the next session opens.

**Real IO** (same file, not covered by unit tests; verified on hardware in Task 7):

```rust
use evdev::{uinput::VirtualDevice, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, RawDevice};
use i2cdev::core::{I2CMessage, I2CTransfer};
use i2cdev::linux::{LinuxI2CBus, LinuxI2CMessage};

const TOUCHPAD_PREFIXES: &[&str] = &["ASUE", "ASUF", "ASUP", "ELAN"];

pub struct EvdevIo {
    events: evdev::raw_stream::EventStream,
    uinput: VirtualDevice,
    i2c: Option<LinuxI2CBus>,
    addr: u16,
    area: Area,
    layout: &'static Layout,
}

/// Finds the NumberPad touchpad (by name), its i2c bus, and sets up the virtual keyboard.
pub fn open_real() -> std::io::Result<EvdevIo> {
    let (path, name) = std::fs::read_dir("/sys/class/input")?.flatten()
        .filter_map(|e| { let n = e.file_name().into_string().ok()?; n.starts_with("event").then_some(n) })
        .find_map(|n| {
            let name = std::fs::read_to_string(format!("/sys/class/input/{n}/device/name")).ok()?.trim().to_string();
            (name.ends_with("Touchpad") && TOUCHPAD_PREFIXES.iter().any(|p| name.starts_with(p))).then(|| (format!("/dev/input/{n}"), name))
        })
        .ok_or_else(|| std::io::Error::other("no NumberPad touchpad"))?;
    let layout = layout_for(&name).ok_or_else(|| std::io::Error::other(format!("no NumberPad layout for {name}")))?;
    let dev = RawDevice::open(&path)?;
    let abs: Vec<_> = dev.get_absinfo()?.collect();
    let get = |code: AbsoluteAxisCode| abs.iter().find(|(c, _)| *c == code).map(|(_, i)| (i.minimum(), i.maximum()));
    let (minx, maxx) = get(AbsoluteAxisCode::ABS_MT_POSITION_X).ok_or_else(|| std::io::Error::other("touchpad has no X"))?;
    let (miny, maxy) = get(AbsoluteAxisCode::ABS_MT_POSITION_Y).ok_or_else(|| std::io::Error::other("touchpad has no Y"))?;
    let mut keys = AttributeSet::<KeyCode>::new();
    for row in layout.keys { for &k in *row { keys.insert(KeyCode(k)); } }
    keys.insert(KeyCode(KEY_NUMLOCK));
    let uinput = VirtualDevice::builder()?.name("armoury numberpad").with_keys(&keys)?.build()?;
    // the i2c bus is the i2c-N ancestor of the touchpad's i2c device (ACPI name = first word of the evdev name)
    let acpi = name.split(' ').next().unwrap_or_default();
    let bus = std::fs::canonicalize(format!("/sys/bus/i2c/devices/i2c-{acpi}")).ok()
        .and_then(|p| p.parent().and_then(|b| b.file_name()).and_then(|b| b.to_str()).map(|b| format!("/dev/{b}")));
    let i2c = bus.and_then(|b| LinuxI2CBus::new(&b).map_err(|e| eprintln!("armouryd: NumberPad i2c {b}: {e}")).ok());
    Ok(EvdevIo { events: dev.into_event_stream()?, uinput, i2c, addr: super::layout::i2c_address(&name), area: Area { minx, maxx, miny, maxy }, layout })
}

#[async_trait::async_trait]
impl NumpadIo for EvdevIo {
    async fn next(&mut self) -> std::io::Result<Raw> {
        loop {
            let ev = self.events.next_event().await?;
            let raw = match (ev.event_type(), ev.code()) {
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_SLOT.0 => Raw::Slot(ev.value()),
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_TRACKING_ID.0 => Raw::TrackingId(ev.value()),
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_POSITION_X.0 => Raw::X(ev.value()),
                (EventType::ABSOLUTE, c) if c == AbsoluteAxisCode::ABS_MT_POSITION_Y.0 => Raw::Y(ev.value()),
                (EventType::SYNCHRONIZATION, 0) => Raw::Syn,
                _ => continue,
            };
            return Ok(raw);
        }
    }
    fn area(&self) -> Area { self.area }
    fn layout(&self) -> &'static Layout { self.layout }
    fn grab(&mut self, on: bool) -> std::io::Result<()> {
        if on { self.events.device_mut().grab() } else { self.events.device_mut().ungrab() }
    }
    fn light(&mut self, v: u8) -> std::io::Result<()> {
        let Some(bus) = self.i2c.as_mut() else { return Err(std::io::Error::other("no i2c bus")) };
        let data = backlight_packet(v);
        bus.transfer(&mut [LinuxI2CMessage::write(&data).with_address(self.addr)]).map(|_| ()).map_err(std::io::Error::other)
    }
    fn key(&mut self, code: u16, down: bool) -> std::io::Result<()> {
        self.uinput.emit(&[InputEvent::new(EventType::KEY.0, code, down as i32)])
    }
}
```

Notes:
- `VirtualDevice::emit` appends the SYN report itself (check `uinput.rs`; if it doesn't, add `InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0)`).
- If `AbsoluteAxisCode::ABS_MT_POSITION_X.0` isn't a public `u16` field, compare with `AbsoluteAxisCode::ABS_MT_POSITION_X.0 as u16` or `.0` per the crate's `constants.rs`.
- Make `mod.rs` export nothing more than the modules.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS, all four worker tests included. Also `cargo build --release` (the real IO must compile).

- [ ] **Step 5: Commit.**

```bash
git add crates && git commit -m "feat(numpad): worker with evdev/uinput/i2c IO, device-loss handling, Hypr NumLock query"
```

---

### Task 5: Daemon integration, CLI, udev

**Files:** Modify `crates/armouryd/src/ipc.rs`, `crates/armouryd/src/main.rs`, `crates/armoury/src/main.rs`, `packaging/udev/70-omarchy-armoury.rules`, `tests/uninstall_test.sh`.

**Interfaces — Consumes:** `worker::{Cmd, run_worker, open_real}`, `NumpadState`, `NumpadConfig`.
**Produces:**
- `Daemon::with_numpad(self: Arc<Self>, tx: mpsc::Sender<numpad::worker::Cmd>, state: Arc<std::sync::Mutex<NumpadState>>) -> Arc<Self>`: same shape as `with_hypr` (`Arc::try_unwrap`, set the fields, `Arc::new`).
- Requests `set_numpad` and `set_numpad_config` handled.
- Snapshot `system.numpad`.
- The tick sends `Cmd::Env` whenever active/touchpad changes (and once at start).
- `KeyAction::ToggleNumpad` sends `Cmd::Set(!on)`.
- CLI `armoury numpad [on|off]`.

- [ ] **Step 1: Failing tests** (ipc tests):

```rust
    fn numpad_rig(active: bool, toml: &str) -> (SysRig, tokio::sync::mpsc::Receiver<crate::features::numpad::worker::Cmd>, Arc<std::sync::Mutex<armoury_proto::NumpadState>>) {
        let SysRig { d, sys, asusd, svc, hypr, dir } = sys_rig(active, toml);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let state = Arc::new(std::sync::Mutex::new(armoury_proto::NumpadState::Off));
        let d = d.with_numpad(tx, state.clone()); // builders take self: Arc<Self> (like with_hypr)
        (SysRig { d, sys, asusd, svc, hypr, dir }, rx, state)
    }
```

```rust
    #[tokio::test]
    async fn numpad_requests_and_snapshot() {
        use crate::features::numpad::worker::Cmd;
        let (r, mut rx, state) = numpad_rig(true, "");
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_numpad","on":true}))).await.ok);
        let mut got_set = false;
        while let Ok(c) = rx.try_recv() { if matches!(c, Cmd::Set(true)) { got_set = true; } }
        assert!(got_set);
        *state.lock().unwrap() = armoury_proto::NumpadState::On;
        assert_eq!(r.d.refresh().await.system.numpad, armoury_proto::NumpadState::On);
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_numpad_config","idle_dim_secs":0}))).await.ok);
        assert_eq!(r.d.config.lock().await.numpad.idle_dim_secs, 0);
        assert!(!r.d.handle(sreq(serde_json::json!({"cmd":"set_numpad_config","hold_ms":50}))).await.ok, "validated");
    }

    #[tokio::test]
    async fn numpad_refused_in_observe_and_when_touchpad_off() {
        let (o, _rx, _s) = numpad_rig(false, "");
        assert!(o.d.handle(sreq(serde_json::json!({"cmd":"set_numpad","on":true}))).await.error.unwrap().contains("observe"));
        let (r, _rx, _s) = numpad_rig(true, "");
        let off = r.dir.path().join("home/.local/state/omarchy/toggles/hypr");
        std::fs::create_dir_all(&off).unwrap();
        std::fs::write(off.join("touchpad-disabled-name"), "x").unwrap();
        assert!(r.d.handle(sreq(serde_json::json!({"cmd":"set_numpad","on":true}))).await.error.unwrap().contains("touchpad"));
    }

    #[tokio::test]
    async fn tick_tells_the_worker_about_mode_and_touchpad() {
        use crate::features::numpad::worker::Cmd;
        let (r, mut rx, _s) = numpad_rig(true, "");
        r.d.tick().await;
        let envs: Vec<(bool, bool)> = std::iter::from_fn(|| rx.try_recv().ok()).filter_map(|c| match c { Cmd::Env { active, touchpad_enabled } => Some((active, touchpad_enabled)), _ => None }).collect();
        assert_eq!(envs, [(true, true)]);
        r.d.tick().await;
        assert!(rx.try_recv().is_err(), "unchanged: not re-sent");
    }
```

- [ ] **Step 2: Run** `cargo test -p armouryd --lib numpad_`. Expected: compile errors.

- [ ] **Step 3: Implement.**

**Daemon fields** (and their inits in `new`):
- `numpad_tx: Option<mpsc::Sender<numpad::worker::Cmd>>`
- `numpad_state: Arc<std::sync::Mutex<NumpadState>>` (init `Unavailable`)
- `numpad_env: std::sync::Mutex<Option<(bool, bool)>>`

**Builder** `with_numpad(tx, state)`, shaped like `with_hypr`.

**Helper:**

```rust
    fn numpad_send(&self, c: numpad::worker::Cmd) -> bool {
        self.numpad_tx.as_ref().is_some_and(|tx| tx.try_send(c).is_ok())
    }
```

**Tick:** at the very start of `tick_locked` (before the observe early return), add:

```rust
        let env = (self.mode.get() == ControlMode::Active, self.touchpad_enabled());
        if *self.numpad_env.lock().unwrap() != Some(env) && self.numpad_send(numpad::worker::Cmd::Env { active: env.0, touchpad_enabled: env.1 }) {
            *self.numpad_env.lock().unwrap() = Some(env);
        }
```

**Snapshot:** after `s.system.touchpad = …`, add `s.system.numpad = *self.numpad_state.lock().unwrap();`.

**Handlers** (replace the Task 1 placeholders):

```rust
            Request::SetNumpad { on } => {
                if let Err(r) = self.write_guard().await { return r; }
                if on && !self.touchpad_enabled() && !self.config.lock().await.numpad.allow_when_touchpad_off {
                    return Response::err("the touchpad is off (allow the NumberPad while it's off in Input settings)");
                }
                if *self.numpad_state.lock().unwrap() == NumpadState::Unavailable || !self.numpad_send(numpad::worker::Cmd::Set(on)) {
                    return Response::err("NumberPad not available");
                }
                Response::ok(serde_json::json!({"on": on}))
            }
            Request::SetNumpadConfig { start_brightness, allow_when_touchpad_off, idle_dim_secs, hold_ms } => {
                if let Err(r) = self.write_guard().await { return r; }
                let mut cfg = self.config.lock().await;
                let mut n = cfg.numpad;
                if let Some(v) = start_brightness { n.start_brightness = v; }
                if let Some(v) = allow_when_touchpad_off { n.allow_when_touchpad_off = v; }
                if let Some(v) = idle_dim_secs { n.idle_dim_secs = v; }
                if let Some(v) = hold_ms { n.hold_ms = v; }
                if let Err(e) = n.validate() { return Response::err(e); }
                cfg.numpad = n;
                if let Err(e) = cfg.save(&self.config_path) { return Response::err(format!("save config: {e}")); }
                drop(cfg);
                self.numpad_send(numpad::worker::Cmd::Config(n));
                Response::ok(serde_json::to_value(n).unwrap())
            }
```

**Hotkey:** in `on_hotkey`:

```rust
            KeyAction::ToggleNumpad => {
                let on = *self.numpad_state.lock().unwrap() == NumpadState::On;
                let r = self.handle(Request::SetNumpad { on: !on }).await;
                if !r.ok { self.osd("keyboard", &format!("NumberPad: {}", r.error.unwrap_or_default())).await; }
            }
```

**`main.rs`:** before building the daemon:

```rust
    let (np_tx, np_rx) = tokio::sync::mpsc::channel(16);
    let np_state = std::sync::Arc::new(std::sync::Mutex::new(armoury_proto::NumpadState::Unavailable));
```

- Chain `.with_numpad(np_tx, np_state.clone())` onto the daemon builder (and `.with_hypr(...)`, if the real Hypr is set there; check how the daemon gets `RealHypr` and pass the same `Arc` to the worker).
- After `poll_loop` is spawned:

```rust
    let np_cfg = daemon.config.lock().await.numpad;
    tokio::spawn(armouryd::features::numpad::worker::run_worker(
        || armouryd::features::numpad::worker::open_real().map(|io| Box::new(io) as Box<dyn armouryd::features::numpad::worker::NumpadIo>),
        np_rx, np_state, std::sync::Arc::new(armouryd::hw::hypr::RealHypr), np_cfg));
```

**CLI:**
- Add `Numpad { #[arg(value_parser = ["on", "off"])] state: Option<String> }` with a doc comment ("Show or switch the NumberPad").
- `None` → print `Status.system.numpad`.
- `Some(s)` → `call(&Request::SetNumpad { on: s == "on" })`.

**udev** (`packaging/udev/70-omarchy-armoury.rules`): append:

```
# NumberPad (armouryd, active mode only): read the touchpad (and grab it while the
# NumberPad is on), write its backlight over i2c, type keys through a virtual keyboard.
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{name}=="ASUE* Touchpad", TAG+="uaccess"
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{name}=="ASUF* Touchpad", TAG+="uaccess"
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{name}=="ASUP* Touchpad", TAG+="uaccess"
SUBSYSTEM=="input", KERNEL=="event*", ATTRS{name}=="ELAN* Touchpad", TAG+="uaccess"
SUBSYSTEM=="i2c-dev", TAG+="uaccess"
KERNEL=="uinput", OPTIONS+="static_node=uinput", TAG+="uaccess"
```

**`tests/uninstall_test.sh` check 5:** rename it to "udev rules grant only via uaccess (N-KEY, touchpad, i2c, uinput)". Keep the `! grep -q 'MODE="0666"'` part, and add `grep -q 'ASUE\* Touchpad' …`.

- [ ] **Step 4: Run** `cargo test --workspace && bash tests/uninstall_test.sh`. Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates packaging tests && git commit -m "feat(armouryd): NumberPad requests, snapshot, key action, worker wiring; CLI 'armoury numpad'; udev access"
```

---

### Task 6: UI — popup tile and Input page section

**Files:** Modify `plugin/Widget.qml`, `plugin/InputPage.qml`.

**Interfaces — Consumes:**
- `snap.system.numpad` (`"unavailable" | "off" | "on"`)
- requests `set_numpad`, `set_numpad_config`, `config` (`numpad` object)
- the key action `toggle_numpad`

- [ ] **Step 1: Popup tile.** In `Widget.qml`'s quick-toggle `Grid`, after the Touchpad `Toggle`, add:

```qml
            Toggle {
              icon: "󰎠"; label: "NumberPad"
              visible: grid.sys.numpad !== undefined
              on: grid.sys.numpad === "on"
              usable: root.active && grid.sys.numpad !== "unavailable"
              onClicked: client.run({ cmd: "set_numpad", on: !on })
            }
```

(`Toggle` already has `usable`, as the Music tile shows.)

- [ ] **Step 2: Input page section.**
- In `InputPage.qml`, add `{ label: "Toggle NumberPad", value: "toggle_numpad" }` to `actions`.
- Add a `readonly property var np: cfg.numpad || ({ start_brightness: 8, allow_when_touchpad_off: false, idle_dim_secs: 60, hold_ms: 1000 })`.
- Add `function setNp(key, v) { var r = { cmd: "set_numpad_config" }; r[key] = v; client.run(r, function() { root.reload() }) }`.
- Before the `KEYBOARD BACKLIGHT WHEN IDLE` section, add:

```qml
    Section { text: "NUMBERPAD"; fg: root.fg }
    Text {
      visible: root.sys.numpad === "unavailable" || root.sys.numpad === undefined
      width: parent.width
      wrapMode: Text.WordWrap
      text: root.usable ? "No NumberPad touchpad found." : "The NumberPad works while armouryd is in control (Take over)."
      color: root.fg; opacity: 0.6; font.family: root.fontFamily; font.pixelSize: Style.font.caption
    }
    ChoiceRow {
      fg: root.fg
      label: "NumberPad"
      usable: root.usable && root.sys.numpad !== "unavailable"
      options: [{ label: "On", value: true }, { label: "Off", value: false }]
      value: root.sys.numpad === "on"
      onChosen: function(v) { root.client.run({ cmd: "set_numpad", on: v }) }
    }
    ValueSlider {
      fg: root.fg; label: "Brightness when it turns on"; unit: ""
      minimum: 1; maximum: 8
      value: root.np.start_brightness
      usable: root.usable
      onCommitted: function(v) { root.setNp("start_brightness", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Dark after no touch (0 = never)"; unit: "s"
      minimum: 0; maximum: 600; step: 10
      value: root.np.idle_dim_secs
      usable: root.usable
      onCommitted: function(v) { root.setNp("idle_dim_secs", v) }
    }
    ValueSlider {
      fg: root.fg; label: "Hold time to toggle"; unit: "ms"
      minimum: 300; maximum: 3000; step: 100
      value: root.np.hold_ms
      usable: root.usable
      onCommitted: function(v) { root.setNp("hold_ms", v) }
    }
    ChoiceRow {
      fg: root.fg
      label: "While the touchpad is off"
      usable: root.usable
      options: [{ label: "NumberPad off too", value: false }, { label: "Allow NumberPad", value: true }]
      value: root.np.allow_when_touchpad_off === true
      onChosen: function(v) { root.setNp("allow_when_touchpad_off", v) }
    }
```

- [ ] **Step 3: Compile check** with `./tests/qml_check.sh`. Expected: all OK.

- [ ] **Step 4: Runtime probe.** A probe shell (same pattern as Plan 5b's) creates `InputPage` with a fake client whose `config` answer includes `numpad: {start_brightness: 5, allow_when_touchpad_off: false, idle_dim_secs: 30, hold_ms: 1000}` and whose snapshot has `system.numpad: "off"`. Log the sliders' values: expected `5`, `30`, `1000`. Call `setNp("idle_dim_secs", 0)` and log the request the fake received: expected `{"cmd":"set_numpad_config","idle_dim_secs":0}`.

- [ ] **Step 5: Commit.**

```bash
git add plugin && git commit -m "feat(ui): NumberPad quick toggle and Input settings (start brightness, idle dim, hold time, touchpad-off rule)"
```

---

### Task 7: Install and verify on the laptop

**Files:** none (fixes go to the owning task's files, with a failing test first).

- [ ] **Step 1: Install.**
  1. `cargo build --release`
  2. Install `armouryd` and `armoury` to `~/.local/bin`.
  3. `sudo install -Dm644 packaging/udev/70-omarchy-armoury.rules /etc/udev/rules.d/`
  4. `sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=input --subsystem-match=i2c-dev --subsystem-match=misc --action=change`. This needs the user's password; ask them to run it with `!`, or use `pkexec`.
  5. `systemctl --user restart armouryd && omarchy-restart-shell`.
  6. Check `armoury status` shows observe, and `armoury numpad` prints `unavailable` (observe mode).

- [ ] **Step 2: Takeover session with the user** (they touch the pad). Each check:
  1. `armoury takeover`, then `armoury numpad` → `off`.
  2. Hold the top-right icon for 1 s: light at max, cursor frozen, `armoury numpad` → `on`.
  3. Type `123.45+6↵` into a text field (e.g. a terminal `cat`): correct characters, including with NumLock turned off beforehand.
  4. Tap top-left: the brightness steps.
  5. Wait 60 s: it goes dark. The next touch lights it without typing.
  6. Omarchy touchpad off (`omarchy-toggle-input-device touchpad off`): the NumberPad turns off, and the hold does nothing. Touchpad back on.
  7. Hold to turn off: the cursor works again.
  8. The popup NumberPad tile and the Input page controls work.
  9. `armoury handback`: G-Helper's NumberPad works again (hold its icon once).

- [ ] **Step 3: Final review and merge.**
  - Whole-branch review (`main..plan-8-numpad`) on the most capable model, with the Review Focus list.
  - One fix pass with tests.
  - Fast-forward `main`; push when the user says so.
