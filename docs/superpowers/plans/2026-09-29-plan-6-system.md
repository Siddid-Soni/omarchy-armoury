# Plan 6 — Battery, display, toggles, sleep, AC/battery auto-switch

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** everything G-Helper still does for this user outside performance/lighting: battery charge limit + one-shot full charge + battery info, refresh rate (also per power source), Panel Overdrive, gamma, touchpad / boot sound / clamshell toggles, s2idle/deep sleep, and per-power-source performance mode — so G-Helper can be retired.

**Architecture:**
- Battery limit, one-shot, profile-on-AC/battery: asusd `xyz.ljones.Platform` (`ChargeControlEndThreshold y`, `OneShotFullCharge()`, `PlatformProfileOnAc/OnBattery u`, `ChangePlatformProfileOnAc/OnBattery b`). asusd already auto-switches the profile on power changes and persists the limit; armouryd configures it instead of duplicating it.
- Panel OD, boot sound: asusd armoury attributes `panel_overdrive`, `boot_sound` (`CurrentValue` readable and writable on this machine).
- Refresh rate, touchpad: Hyprland runtime via `hyprctl eval` with Lua (`hl.monitor{…}` as in `~/.config/hypr/monitors.lua`; legacy `hyprctl keyword` is rejected by this Hyprland). Per-source refresh applied on AC/battery change.
- Gamma: `hyprctl hyprsunset gamma <n>` (Omarchy runs hyprsunset).
- Clamshell: armouryd holds `systemd-inhibit --what=handle-lid-switch sleep infinity` while enabled and on AC; kills it otherwise.
- Sleep mode: `armoury-root mem-sleep <s2idle|deep>` (allowlisted, active flag required); armouryd re-applies the configured mode at start in active mode (the kernel resets it every boot).
- Camera: this model has no webcam (no UVC device); the toggle is hidden when `/sys/bus/usb/drivers/uvcvideo` has no bound device. Not built beyond detection.

**Facts (this machine):** BAT0 energy_full 64.1 Wh / design 90.0 Wh (71 % health), cycle_count 0 (firmware reports none), voltage_now, power_now, capacity; display eDP-1 2560x1440 at 240 or 60 Hz, scale 1.6; touchpad `asue1403:00-04f3:319a-touchpad`; boot_sound 0; panel_od 1; `/sys/power/mem_sleep` `[s2idle] deep`.

## Global Constraints
- Writes only in active mode; asusd calls `with_retry`; root commands allowlisted + active flag.
- Only offer what exists (refresh rates from `hyprctl monitors -j` availableModes; camera only if present).
- Config (`[system]`, `[power_source.ac]`, `[power_source.battery]`) saved only after success.

## Review Focus
1. Charge limit outside 20–100 → refused (asusd and firmware accept 20–100).
2. Refresh rate not in availableModes → refused, nothing sent to Hyprland.
3. AC↔battery flips → refresh follows stored per-source rate exactly once per flip.
4. Clamshell: unplug while enabled → inhibitor released; re-plug → re-acquired; daemon exit → no orphan (child killed on drop).
5. mem-sleep with junk → refused by root.

## Tasks
1. **Proto:** `BatteryInfo { capacity, status, health_pct, full_wh, design_wh, cycles, voltage_v, draw_w, time_left_min, charge_limit }` (from sysfs, observe-safe, in `Snapshot.battery_info`); `DisplayInfo { output, width, height, refresh_hz, rates: Vec<f32>, scale }` (Hyprland, in `Snapshot.display`); `SystemState { boot_sound, panel_od, touchpad_enabled, clamshell, mem_sleep, camera_present }`; requests `SetChargeLimit{percent}`, `OneShotCharge`, `SetRefresh{hz}`, `SetPanelOd{on}`, `SetGamma{percent}`, `SetTouchpad{on}`, `SetBootSound{on}`, `SetClamshell{on}`, `SetSleepMode{mode}`, `SetSourceProfile{ac: Option<Profile>, battery: Option<Profile>}`, `SetSourceRefresh{ac: Option<f32>, battery: Option<f32>}`. Tests: wire formats.
2. **Battery info (pure):** `battery_info(sys) -> BatteryInfo` from energy_* / charge_* (µWh/µAh with voltage_min_design fallback), health %, time left = energy_now/power_now when discharging. Test with this machine's numbers (health 71).
3. **Hyprland client:** trait `Hypr { monitors() -> Result<Vec<DisplayInfo>>, eval(lua) -> Result<()>, sunset_gamma(pct) -> Result<()> }`; `RealHypr` shells out to `hyprctl -j monitors` / `hyprctl eval` / `hyprctl hyprsunset gamma`; `lua_monitor(d, hz) -> String` builds `hl.monitor({ output = "eDP-1", mode = "2560x1440@60", position = "auto", scale = 1.6 })`; `lua_touchpad(name, on)`. Tests: Lua strings, rate validation.
4. **asusd extensions:** `Asusd` gains `set_charge_limit(u8)`, `one_shot_charge()`, `set_source_profiles(ac, battery)` (sets the OnAc/OnBattery profile and turns the Change* flag on for each given), `armoury_get/armoury_set_value(attr, i32)` for panel_overdrive/boot_sound. Fake + tests.
5. **Root mem-sleep + clamshell holder:** `armoury-root mem-sleep <s2idle|deep>` (validated, writes `/sys/power/mem_sleep`); armouryd `Clamshell` holder owning a `tokio::process::Child` with `kill_on_drop`. Tests: arg validation; holder acquire/release with a fake spawner.
6. **Daemon:** handlers for all requests (guard, validate, apply, save); tick: per-source refresh on AC change, clamshell acquire/release on AC change, mem-sleep re-apply once per active start; `Snapshot.system`, `.battery_info`, `.display`. Tests per Review Focus.
7. **CLI:** `armoury battery [limit N|oneshot]`, `armoury display [refresh HZ|od on|off|gamma N]`, `armoury toggle touchpad|bootsound|clamshell on|off`, `armoury sleep s2idle|deep`, `armoury auto profile --ac X --battery Y`, `armoury auto refresh --ac 240 --battery 60`; status lines. Tests: parsers.
8. **On-device:** limit 80 (unchanged), refresh 60→240, OD toggle and back, touchpad off/on, sleep mode deep→s2idle, auto profile read back from asusd; handback.
