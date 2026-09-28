# Omarchy Armoury — Design (v1 rewrite)

Date: 2026-09-29
Status: approved in brainstorming, pending written-spec review

## 1. Goal

An Omarchy extension that replaces G-Helper on ASUS ROG laptops: every
g-helper-linux feature that applies to the machine, plus music-reactive
lighting and Keystone actions, in a native Omarchy bar popup and window.

Reference machine: ROG Strix G533ZW (i9-12th gen + Iris Xe, RTX 3070 Ti,
MUX present, N-KEY `0b05:19b6`, touchpad NumberPad `ASUE1403 04F3:319A`,
Keystone slot). Everything is capability-probed; nothing is hard-coded to
this model.

Success: G-Helper can be uninstalled with no lost functionality the user
selected, and the logo + lightbar light up.

### Constraints

- G-Helper stays installed and in use until v1 is finished. The new stack
  must never fight it (see §3.4).
- Old code is removed at the start of implementation (baseline preserved in
  git commit `47e2891`).
- GPU Hybrid↔Integrated must use Omarchy's built-in
  `omarchy-toggle-hybrid-gpu`; Hybrid↔Ultimate uses `supergfxctl`.

### Out of scope (v1)

Per-key lighting editor (the per-key frame layer is built, editor later),
audio DSP (mic noise reduction/EQ), peripherals (mice), AniMe/Slash,
XG Mobile, Ally, MiniLED, run-command-on-mode-switch, automated
Integrated↔Ultimate, window layout options, Keystone identity (NFC).

## 2. Key findings that shape the design

- **asusd lights only the keyboard** because `/usr/share/asusd/aura_support.ron`
  lists `G533Z` with `power_zones: [Keyboard]`. Fix: `[Keyboard, Lightbar, Logo]`
  and delete `/etc/asusd/aura_19b6.ron` so asusd regenerates it. The file is
  package-owned and there is no `/etc` override, so a pacman hook re-applies
  the fix after every asusctl upgrade.
- **Keystone presence** is exposed by the kernel at
  `/sys/devices/platform/asus-nb-wmi/keystone` (1 = inserted).
- **dGPU in Hybrid already reaches D3cold** (NVIDIA RTD3,
  `DynamicPowerManagement: 3`), so Integrated gains little on this machine.
- **Black-screen state:** `gpu_mux_mode=0` with `dgpu_disable=1`.
- `/sys/power/mem_sleep` offers `s2idle` and `deep`.
- Intel undervolt is via MSR 0x150 and is usually BIOS-locked on 12th gen.

## 3. Architecture

```
omarchy-armoury/
├─ plugin/                 QML, Omarchy plugin id "asus.armoury"
│   ├─ manifest.json       kinds: bar-widget, panel, service
│   ├─ Widget.qml          bar icon + quick popup
│   ├─ Window.qml          full window (panel), dashboard tiles + detail pages
│   └─ Service.qml         single socket client; shared state for both UIs
├─ armouryd/               Rust, systemd --user service; the only hardware owner
│   ├─ hw/                 traits + impls: asusd D-Bus, supergfxd D-Bus,
│   │                      sysfs, hidraw, evdev/uinput, i2c, NVML (via root)
│   ├─ features/           profile, fans, power, gpu, aura, music, numpad,
│   │                      keys, fnlock, keystone, powersource, battery,
│   │                      display, toggles, sleep
│   └─ ipc/                Unix socket at $XDG_RUNTIME_DIR/armoury.sock,
│                          newline-delimited JSON requests + event stream
├─ armoury-root/           Rust, root binary, fixed command allowlist (§3.3)
├─ armoury/                Rust CLI (`armoury …`), thin IPC client
└─ packaging/              polkit rule, udev rules, systemd units, pacman hook,
                           install.sh, uninstall.sh
```

### 3.1 Process roles

- **armouryd** owns all hardware access and all state. It keeps running when
  omarchy-shell restarts, so auto-switch, Keystone, NumberPad and music keep
  working.
- **QML** only sends requests and renders state/events. No hardware access.
- **armoury CLI** is for keybinds, hypridle hooks, scripts and testing.

### 3.2 Backends

| Area | Backend |
|---|---|
| Profiles, fan curves, battery limit/one-shot, Aura, power limits, Panel OD, boot sound, NVIDIA boost/temp target | asusd D-Bus |
| GPU mode | supergfxd D-Bus; Omarchy toggle script for Integrated |
| Keystone, GPU readback, battery info, temps, fan RPM | sysfs / hwmon (read) |
| Music frames | asusd per-key direct if ≥30 fps sustained; else hidraw `/dev/hidraw*` (N-KEY) only while music is on |
| Hotkeys, NumberPad touch input | evdev (udev ACL) |
| NumberPad digits, software Fn-lock | uinput |
| NumberPad backlight | i2c-dev (udev ACL) |
| CPU boost (`intel_pstate/no_turbo`), EPP | sysfs via udev chmod rules |
| Refresh rate, touchpad enable, gamma | Hyprland IPC / hyprsunset |
| Power-source changes | UPower D-Bus |

Every call to asusd/supergfxd/NVML has a timeout (3 s) and one retry.

### 3.3 armoury-root

Invoked via `pkexec`; polkit rule grants the installing user without a
password. Commands (nothing free-form, every argument re-validated):

- `undervolt probe` / `undervolt set <mV>` (range −150…0)
- `nv-clocks set <core_off> <mem_off>` / `nv-clocks lock <core_mhz|off> <mem_mhz|off>`
- `mem-sleep <s2idle|deep>`
- `camera <on|off>`
- `asusd-support-fix` (idempotent; also run by the pacman hook)
- `takeover` / `handback` (§3.4)

### 3.4 Coexistence with G-Helper

- armouryd starts in **observe mode**: reads everything, writes nothing,
  background features (music, NumberPad, keys, auto-switch, Keystone actions)
  inactive.
- `armoury takeover`: stops G-Helper, starts asusd, switches armouryd to
  active mode.
- `armoury handback`: switches armouryd to observe, stops asusd, restarts
  G-Helper.
- Mode is persisted and shown in the UI header.

### 3.5 Config

`~/.config/omarchy-armoury/config.toml`: per-mode settings (fans, limits,
CPU, NVIDIA, undervolt), AC/battery rules, lighting, key bindings, NumberPad,
Keystone actions, toggles, sleep mode. Written only after a successful apply.

## 4. Features

### 4.1 Performance & fans

- Modes Silent / Balanced / Turbo (asusd profile).
- Per mode, auto-applied on mode change:
  - fan curves: 8 points each for CPU and GPU fans (validated monotonic)
  - PL1 / PL2
  - CPU boost on/off, EPP
  - NVIDIA Dynamic Boost and temp target (asusd)
  - NVIDIA core/mem offsets, clock lock, VRAM lock (NVML via armoury-root)
  - undervolt (only if `undervolt probe` says unlocked)
- Optional re-apply of power limits every N seconds (0 = off).
- Manual fixed fan RPM with hysteresis, only if the firmware exposes a fan
  target; hidden otherwise.
- Monitor: CPU/GPU temps, fan RPM, power draw, GPU status (clocks, P-state,
  VRAM, throttle).
- Only controls whose attributes exist are rendered.

### 4.2 Auto-switch on power source

AC and battery each hold: performance mode, refresh rate, keyboard
brightness. On change, armouryd applies the set. GPU is never switched
automatically; a notification suggests it.

### 4.3 GPU

| Mode | mux | dgpu_disable | supergfxd |
|---|---|---|---|
| Integrated | 1 | 1 | Integrated |
| Hybrid | 1 | 0 | Hybrid |
| Ultimate | 0 | 0 | AsusMuxDgpu |

| From → To | Path | Reboots |
|---|---|---|
| Hybrid ↔ Integrated | `omarchy-toggle-hybrid-gpu` in a floating terminal | 1 |
| Hybrid ↔ Ultimate | `supergfxctl -m AsusMuxDgpu` / `-m Hybrid` | 1 |
| Integrated ↔ Ultimate | Not direct. UI explains the two manual steps via Hybrid and offers the first step | 2, manual |

- Guard: refuse any action whose result could be mux=0 with dgpu_disable=1,
  including queuing Integrated while Ultimate is pending (kernel
  `pending_reboot` plus our own pending record keyed to boot ID).
- Pending state is shown ("→ Ultimate after reboot") and conflicting
  buttons are disabled.
- dGPU users view: processes holding `/dev/nvidia*`, shown before switching
  to Integrated.

### 4.4 Lighting

- Power zones keyboard / lightbar / logo × boot / awake / sleep / shutdown.
- asusd effects: Static, Breathe, RainbowCycle, RainbowWave, Star, Rain,
  Highlight, Laser, Ripple, Pulse, Comet, Flash; colour1/colour2, speed,
  direction.
- Brightness off/low/med/high, separate for AC and battery.
- Backlight timeout via hypridle listener calling `armoury kbd idle|resume`;
  "keep on" disables it.
- **Music lighting:** PipeWire capture of the default sink monitor
  (`pipewire` crate) → FFT (`rustfft`) → ~30 fps per-key frames across
  keyboard, lightbar, logo. Styles: Spectrum (bass→treble left→right),
  Pulse (one colour, brightness follows loudness). On stop the previous
  effect is restored.
- The per-key frame layer is a standalone module so the later per-key editor
  can reuse it.

### 4.5 Keys

- Hotkey device (asus-nb-wmi) read via evdev. Bindable keys: ROG key,
  M-keys, Fn+F5.
- Actions: cycle mode, open window, open popup, toggle music, cycle effect,
  cycle keyboard brightness, custom command.
- Defaults: ROG → open window, Fn+F5 → cycle mode.
- Fn-lock: firmware `fn_lock` attribute if present; else opt-in software
  Fn-lock (grab keyboard, remap F1–F12 ↔ media via uinput).

### 4.6 NumberPad

- Touchpad `ASUE1403 04F3:319A` via evdev. Layout 5×4 (G533); layouts are
  data tables so other models can be added.
- Toggle: 1 s hold in top-right corner, UI toggle, or bindable key.
- While on:
  - backlight on via i2c, adjustable brightness
  - grid taps become uinput key events
  - pointer disabled through Hyprland device config, re-enabled on exit

### 4.7 Toggles, sleep, battery, display

- Touchpad (Hyprland), camera (`armoury-root camera`), boot sound (asusd),
  clamshell mode (logind lid inhibitor held while on AC and enabled).
- Sleep: s2idle/deep via `armoury-root mem-sleep`, re-applied at daemon start.
- Battery: limit (60/80/100/custom), one-shot full charge, info (health =
  full/design, cycles, voltage, draw, time remaining).
- Display: refresh rate (also per AC/battery), Panel OD, gamma/brightness via
  hyprsunset.

### 4.8 Keystone

- Watch `/sys/devices/platform/asus-nb-wmi/keystone`: uevent if emitted,
  else 1 s poll.
- Configurable actions on insert and on remove: set mode, apply lighting
  preset, lock screen (remove), custom command.
- Presence only; which Keystone it is needs NFC (later phase).

## 5. UI

- **Bar icon:** current mode + CPU temp.
- **Popup (option B):**
  - header: model + Keystone state
  - segmented rows: Mode, GPU
  - 3×2 quick-toggle grid: keyboard light, music, NumberPad, touchpad,
    camera, Fn-lock
  - status lines: battery, temps/fan
  - "Open Armoury ›"
- **Window (option C):**
  - header strip: model, mode, GPU, Keystone, observe/active indicator
  - dashboard tiles: Fans, Lighting, Battery, CPU/GPU, Input, System
  - each tile opens a detail page:
    - Fans: draggable curves; Shift drags all points
    - CPU/GPU: tuning, undervolt, dGPU users, GPU status
    - Lighting: zones, effects, music
    - Input: keys, Fn-lock, NumberPad, touchpad
    - System: sleep, display, clamshell, camera, boot sound, AC/battery
      rules, Keystone
    - Battery: limit, one-shot, info
- Colours come from the active Omarchy theme.
- Missing backend → banner with a fix action (e.g. "asusd not running —
  Takeover"). armouryd down → "daemon offline" + Start.

## 6. Error handling

- A failed write surfaces the real error (UI + notification) and the control
  reverts to the value read back from hardware.
- armoury-root re-validates every argument; the GPU guard runs in both
  armouryd and armoury-root.
- Music and NumberPad workers restart up to 3 times/min, then disable
  themselves with an error.
- supergfxd wedges are handled by the timeout/retry; UI never blocks.

## 7. Testing

- **Rust unit tests:**
  - fan-curve validation
  - GPU transition/guard table
  - config round-trip
  - HID packet builders, byte-exact against g-helper-linux
  - FFT→frame mapping
  - NumberPad hit-testing
  - Keystone state machine
- **Fake hardware:** feature logic tests run against fake asusd, supergfxd
  and sysfs implementations of the `hw` traits.
- **On device:** scripted checklist per feature, run in takeover mode;
  `armoury handback` afterwards.
- **QML:** `qmllint`, `omarchy plugin validate`.

## 8. Install

- `install.sh`:
  1. cargo build --release
  2. binaries to `~/.local/bin`, `armoury-root` to `/usr/local/lib/omarchy-armoury/`
  3. polkit rule, udev rules, systemd units, pacman hook, asusd support fix
     (one sudo prompt)
  4. register plugin
- `uninstall.sh` reverses all of it, including restoring the asusd support
  file from the package.

## 9. Build order

1. Core: daemon, IPC, CLI, observe mode, takeover/handback, asusd fix.
2. Performance & fans, including undervolt and NVIDIA clocks via root helper.
3. GPU.
4. Lighting (zones, effects, brightness, timeout).
5. Popup + window.
6. Battery, display, toggles, sleep, auto-switch.
7. Keys + Fn-lock.
8. NumberPad.
9. Music lighting.
10. Keystone.
