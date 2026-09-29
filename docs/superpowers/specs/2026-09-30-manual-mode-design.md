# Manual mode — design

Date: 2026-09-30 · Status: approved in chat, awaiting spec review
Extends: `2026-09-29-omarchy-armoury-design.md`

## Intent

What the user asked for:

- A fourth mode, **Manual**, next to Silent / Balanced / Turbo.
- Several saved manual profiles, switched inside Manual's settings.
- Fan curves can be changed **only** in Manual; the other modes stay on
  firmware auto.
- CPU/GPU tuning is available only through Manual and belongs to the
  manual profile.
- "Mode on AC / on battery" can pick Manual too.

Success: Silent/Balanced/Turbo behave exactly like stock firmware. Manual
applies the active profile's curves, limits and GPU settings and keeps them
across reboots and takeovers. Plugging in or unplugging can move between a
stock mode and Manual.

## 1. Model

- **Stock modes** (`quiet`, `balanced`, `performance`):
  - fan curves off (firmware auto) and no custom power limits
  - CPU boost on
  - undervolt and NVIDIA clocks at stock
- **Manual** (`manual`): the active manual profile, running on its `base`
  firmware thermal policy.
- A **manual profile** has:
  - `name`: unique and non-empty
  - `base`: `quiet | balanced | performance`, default `performance`
  - `curves`: CPU and GPU (and mid, when the laptop has one), eight
    temperature/percent points each
  - `settings`: the existing `ModeSettings`, i.e. PL1/PL2, CPU boost, EPP,
    undervolt, Dynamic Boost, temperature target, clock offsets/locks

Config (`config.toml`):

```toml
[manual]
enabled = true            # Manual is the current mode (re-applied at start/takeover)
active = "Gaming"         # active profile
[[manual.profiles]]
name = "Gaming"
base = "performance"
settings = { pl1 = 90, pl2 = 120, nv_boost = 25 }
[[manual.profiles.curves]]
fan = "cpu"
temps = [30, 40, 50, 60, 70, 80, 90, 100]
percent = [0, 10, 20, 35, 55, 75, 90, 100]
```

- The old per-mode `[modes]` table is removed, along with the per-mode
  `mode_settings` / `set_mode_settings` requests. Current configs have it
  empty, so there is no migration.
- A leftover `[modes]` table is accepted on load and ignored, with one log
  line, so the config never becomes `config.toml.bad` over this change.
- With no profiles, the first time Manual is entered it creates
  `"Manual 1"`: base Turbo, curves copied from asusd's current Turbo curves,
  empty settings.

## 2. Applying

Everything below happens only in active mode.

**Entering a stock mode X:**

1. `set_profile(X)`
2. Disable custom fan curves for X.
3. Reset to stock:
   - power limits: `set-limits` with no values, `cpu_boost=on`
   - undervolt `0`, but only if it was changed
   - NVIDIA clocks at stock, but only if they were changed and the dGPU is
     awake

**Entering Manual with profile P:**

1. `set_profile(P.base)`, then wait for the policy to settle (existing
   `POLICY_SETTLE`).
2. Write P's curves into asusd's slot for `P.base` and enable them.
3. `apply_mode(P.base, P.settings)`: the existing path (re-assert mode,
   EPP, curves on, PPT, undervolt, NVIDIA).

**Saving the active profile while in Manual** re-applies it at once.

**Following changes made elsewhere.** The poll sees firmware profile F:

- In Manual and `F == base`: nothing to do. Periodic re-apply works as today.
- In Manual and `F != base`: something else changed the mode. Leave Manual
  (`enabled = false`), treat F as the chosen stock mode and apply stock F.
  armouryd follows the change instead of fighting it.
- Not in Manual: stock handling for F, which ensures its curves are off.

**At start / takeover:** if `manual.enabled`, enter Manual; otherwise
stock-apply the current profile.

**Power source.** armouryd now does the AC/battery mode switch itself; asusd
does not know about Manual.

- Config `system.profile_ac` / `profile_battery` become
  `Option<ModeChoice>`, where `ModeChoice = quiet | balanced | performance | manual`.
- On a source flip, armouryd enters the configured mode.
- `set_source_profile` stops calling asusd and turns off asusd's own
  `change_platform_profile_on_ac/battery`, so the two never race.
- Fn+F5's `cycle_mode` action cycles Silent → Balanced → Turbo → Manual.

## 3. Requests (proto)

- `set_profile { profile }`: `profile` also accepts `manual`.
- `manual_profiles` returns `{ enabled, active, profiles: [ManualProfile] }`.
- `save_manual_profile { profile: ManualProfile, original_name?: String }`
  creates, overwrites or renames.
  - Validation: curves are monotonic and 0–100; PL1 ≤ PL2 (the daemon
    enforces it too).
- `activate_manual_profile { name }` sets `active` and enters Manual.
- `delete_manual_profile { name }` refuses to delete the last profile.
  Deleting the active one makes the first remaining profile active.
- `fan_curves` / `set_fan_curve` / `reset_fan_curves` are removed from the
  public protocol. Curves live in manual profiles. Asusd's curve slots are an
  internal detail.
- The snapshot's `perf` gains:
  - `mode`: `quiet | balanced | performance | manual`
  - `manual_profile`: `Option<String>`
- All writes are refused in observe mode, as today.

## 4. UI

- **Bar popup:** MODE row becomes Silent / Balanced / Turbo / Manual.
  - The bar icon for Manual is 󰈐 (fan).
  - Clicking Manual enters it with the active profile.
- **Dashboard:** the Fans and CPU/GPU tiles merge into one **Manual** tile,
  showing the active profile, base mode and fan rpm.
- **Manual page** replaces FansPage and CpuGpuPage:
  - Profile picker (Dropdown) with New / Rename / Delete. New copies the
    current profile.
  - **Base mode** choice: Silent / Balanced / Turbo.
  - Fan graphs (existing FanGraph).
  - CPU and NVIDIA sliders (existing). PL1 and PL2 stay linked: raising PL1
    above PL2 drags PL2 up; lowering PL2 below PL1 drags PL1 down.
  - Buttons:
    - **Save**: saves the profile; re-applies it if it's active and Manual
      is on.
    - **Activate**: shown when this profile isn't the active one, or Manual
      is off.
  - While a stock mode is on, a line reads "Fans are on firmware auto in
    Silent/Balanced/Turbo. Activate a profile to use it."
  - GPU status section (existing).
- **System › Power source:** each row offers Silent / Balanced / Turbo /
  Manual and shows the saved choice.

## 5. Error handling

- Apply failures are reported as today: the request returns the error
  messages and the OSD says "(settings failed)" for key-triggered switches.
  The mode itself still changes.
- Writing the curves fails (asusd down): Manual is still recorded as on. The
  poll retries the apply with the existing retry/backoff.
- Unknown profile name in `activate` / `delete`: error, no change.

## 6. Testing

Daemon unit tests with the fakes:

- stock mode disables its curves and resets limits
- Manual order: base set → curves written → curves enabled → PPT
- an outside mode change drops out of Manual
- start and takeover re-enter Manual
- power-source flip to Manual and back
- profile create / rename / delete rules; the last profile can't be deleted
- config round-trip; a leftover `[modes]` table is ignored

QML:

- compile check
- runtime probe of the Manual page with a fake client: linked PL sliders,
  Activate visibility

Hardware, in one takeover session ending in handback:

- A profile with PL1 = 20 W holds under a CPU load test.
- Switching to Turbo turns fans back to firmware auto (asusd reports curves
  disabled) and removes the cap.
- Unplugging with battery = Silent and AC = Manual switches correctly.

## Out of scope

Key rebinding (M-keys / Fn keys) is designed separately; see the next spec.
