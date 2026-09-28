# Armoury Crate for Omarchy

ASUS laptop control as a native Omarchy bar widget — an Armoury Crate /
G-Helper equivalent built on the daemons Omarchy already preinstalls and
updates: **`asusd`/`asusctl`** and **`supergfxd`/`supergfxctl`**.
No bundled drivers, no telemetry, no second tray app.

Feature scope mirrors [g-helper-linux](https://github.com/utajum/g-helper-linux)
(the Linux port of G-Helper), with one deliberate divergence:
**GPU switching goes through `supergfxctl`**, not `asusctl`'s
`dgpu_disable`/MUX sysfs writes.

## Install

```sh
git clone https://github.com/<you>/omarchy-armoury.git
cd omarchy-armoury
mkdir -p ~/.local/bin
ln -sf "$PWD/bin/omarchy-armoury" ~/.local/bin/omarchy-armoury
omarchy plugin add "$PWD" --enable
# place it on the bar, e.g.:
omarchy bar move asus.armoury --section right
```

Requirements: an ASUS laptop with `asusd` + `supergfxd` running (stock on
Omarchy ASUS installs). On non-ASUS hardware the widget hides itself.
No `sudo` setup — writes go through the daemons' own polkit prompts.

## Usage

Click the bar icon for the panel. Right-click refreshes. Sections:

| Panel section | Backend | Notes |
|---|---|---|
| Performance (Quiet/Balanced/Performance) | `asusctl profile` | g-helper Silent/Balanced/Turbo equivalent |
| GPU Eco / Standard | `omarchy-toggle-hybrid-gpu` (Omarchy's own flow, launched in a terminal) | Config rewrite + reboot, manages delay-start.conf + force-igpu hook |
| GPU Ultimate (MUX) | `supergfxctl -m AsusMuxDgpu` | Offered only when reported in `supergfxctl -s`; reboot always required |
| Battery limit 60/80/100% | `asusctl battery limit` | g-helper longevity limiter |
| Keyboard off/low/med/high + Aura hint | `asusctl leds`, `asusctl aura effect` | Per-key RGB via the aura daemon |
| Logo / lightbar / lid / rear-glow zones | Panel buttons, only for zones the firmware exposes (`omarchy-armoury aura-zones`) | Zone power states (boot+awake vs all-off); single colour via `aura-static <rrggbb> [zone]`. E.g. the Strix G533ZW exposes keyboard only, so no dead buttons are shown. |
| Advanced (CLI escape hatches) | `omarchy-armoury fan-*`, `armoury-*` | Raw attrs, Anime uploads, anything the panel doesn't cover |
| Fans (per active profile) | Panel: curve readout, Enable/Disable, Reset, custom 8-point data | `asusctl fan-curve`; custom format `30c:5%,40c:15%,…` |
| Power limits + MiniLED + Panel OD | Panel numeric rows + Set | `ppt_pl1_spl/pl2/fppt`, `nv_dynamic_boost`, `nv_temp_target`, `mini_led_mode`, `panel_od` |
| Refresh rate | Panel Dropdown + Apply | `hyprctl eval` Lua path (legacy `keyword` is rejected by new Hyprland); verified 240↔60 live |
| Anime Matrix / Slash | Panel, only when hardware present | Enable, brightness, Slash animation picker |
| CPU energy preference | Panel Dropdown (sysfs, no daemon needed) | Governor shown read-only (root-owned node); EPP validated + written to all policies |
| Profile tuning on/off | Panel Toggle | `asusctl profile tuning` |
| One-shot full charge | Panel button | `asusctl battery oneshot` (skips the limiter once) |

The panel uses collapsible sections (advanced ones start collapsed) inside a
scrollable view with a slim scrollbar. If `asusd` isn't running, a banner with
a one-click **Start asusd** button (terminal sudo prompt) replaces guessing —
daemon-backed sections stay hidden until it answers.

Sections appear only when their backend answers: fans/power/Anime/Slash need
`asusd`, refresh rate needs Hyprland. Power rows use per-attribute fallback
ranges — `asusd` validates on set, so a rejected value surfaces as an error,
never a bad write.

### GPU transition matrix

Mirrors Omarchy's `trigger → hardware → Hybrid GPU` flow (`omarchy-toggle-hybrid-gpu`),
which never live-switches: it rewrites `/etc/supergfxd.conf` and reboots (Integrated
boots need the `delay-start.conf` drop-in; suspend needs the `force-igpu` sleep hook).
MUX flips latch in ACPI, so Ultimate always needs a reboot (upstream rule). Implemented
in `Model.transitionFor` (testable) + helper `gpu-set`/`gpu-toggle`:

| From → To | Path |
|---|---|
| Hybrid ↔ Integrated | Omarchy toggle in a terminal (Eco/Standard buttons) |
| Hybrid ↔ Ultimate | Direct `supergfxctl -m` + reboot |
| Integrated → Ultimate | Direct `supergfxctl -m AsusMuxDgpu` + reboot (leaves a harmless 5s delay-start drop-in, cleaned on next toggle run) |
| Ultimate → Integrated | Two-step: direct to Hybrid + reboot, then Eco via the Omarchy toggle (the toggle itself errors on Ultimate, and a direct jump would skip the Integrated boot/suspend support files) |

Caveats: `supergfxd` can wedge and block clients forever — every call goes through
timeout+retry. On newer kernels the `asus-armoury` platform driver can hide
`gpu_mux_mode`, making AsusMuxDgpu vanish from the supported list — the Ultimate
button stays hidden then, by design.

Helper CLI (also useful in scripts/keybinds):

```sh
omarchy-armoury status | profiles | gpu | gpu-support | battery | kbd
omarchy-armoury profile-set Balanced
omarchy-armoury gpu-toggle              # Integrated<->Hybrid via Omarchy flow (terminal)
omarchy-armoury gpu-set AsusMuxDgpu     # Hybrid|AsusMuxDgpu only; reboot required
omarchy-armoury battery-set 80
omarchy-armoury kbd-set med
omarchy-armoury fan-get Balanced
omarchy-armoury armoury-list
```

## g-helper-linux feature map

| g-helper feature | This plugin | How |
|---|---|---|
| Performance modes | ✅ Panel | `asusctl profile set` |
| Custom fan curves (8-pt) | ✅ CLI | `asusctl fan-curve --mod-profile … --fan … --data …` |
| Battery charge limit | ✅ Panel | `asusctl battery limit` |
| GPU Eco/Standard/Ultimate | ✅ Panel (Omarchy toggle + supergfxctl) | Eco/Std delegate to `omarchy-toggle-hybrid-gpu`; Ultimate via `supergfxctl -m` when MUX present (see matrix above) |
| Power limits PL1/PL2/FPPT, Dyn Boost, temp target | ✅ CLI | `asusctl armoury set ppt_pl1_spl …` etc. |
| Panel OD, MiniLED | ✅ CLI | `asusctl armoury set panel_od / mini_led_mode` |
| Keyboard brightness + Aura RGB | ✅ Panel + CLI | `asusctl leds`, `asusctl aura effect …` |
| Anime Matrix / Slash | ✅ CLI | `asusctl anime …`, `asusctl slash …` |
| FnLock, Screenpad, Boot sound | ✅ CLI | `asusctl armoury set …` |
| System monitor / tray / autostart | ➖ deferred to Omarchy | Existing bar widgets cover this; shell autostarts |
| Audio DSP, undervolting (Ryzen SMU/MSR) | ❌ out of scope | Too privileged/fragile for an unsandboxed bar widget |
| ROG Ally handheld, NumberPad, mice | ❌ out of scope v1 | PRs welcome; Ally needs `asusctl` handheld attrs |

## Keystone roadmap (Phase 2, planned)

ASUS ROG Keystone II is a passive **NXP ICODE SLIX2** NFC tag
(ISO 15693 / NFC-V, Type 5, 8-byte UID e.g. `E0:04:01:…`, 80×4-byte blocks)
docked in the laptop's **NXP PN7150** reader. The kernel already exposes the
dock state: `ASUS_WMI_DEVID_KEYSTONE = 0x00120091` in
`include/linux/platform_data/x86/asus-wmi.h` — `PRESENCE_BIT (0x00010000)` =
inserted, `0xFFFFFFFE` = no slot.

Planned work (tracked, not yet implemented — `omarchy-armoury keystone`
currently only probes for the sysfs node):

1. **Presence polling** — read the keystone devstate via asus-wmi sysfs /
   `asusctl` when exposed; show docked/undocked in the panel hero.
2. **UID auth** — match the tag UID over NCI (`neard`, Type 5 / ISO-15693)
   against an enrolled UID in `~/.config/omarchy-armoury/keystone.conf`.
3. **Actions on dock/undock** — profile switch (e.g. dock→Turbo+Ultimate),
   screen lock on undock, ShadowDrive-style dataset decrypt (Linux:
   LUKS keyfile sealed to enrolled UID, never the Windows Shadow Drive blob).
4. **Safety** — writes only via asusd; UID allowlist is auth-theater without
   the SLIX2 password/privacy-mode step, so document that honestly and add
   password-protected blocks if the firmware flow needs it.

References:
- https://github.com/zmr-233/linux-nxp-nfc-fix/blob/main/docs/human/EN/keystone-hardware.md
- `include/linux/platform_data/x86/asus-wmi.h` (`ASUS_WMI_DEVID_KEYSTONE`)

## Validate

```sh
omarchy plugin validate "$PWD"
qmllint -I "$OMARCHY_PATH/shell" Panel.qml
bash -n bin/omarchy-armoury && node --check Model.js
```

## Remove

```sh
omarchy plugin remove asus.armoury
rm ~/.local/bin/omarchy-armoury
```
