# omarchy-armoury

ASUS ROG control for Omarchy. A bar widget and a full settings window
(the `asus.armoury` Omarchy plugin), backed by a Rust daemon (`armouryd`),
a CLI (`armoury`) and a root helper (`armoury-root`).
Design: `docs/superpowers/specs/`.

## Plugin features

### Bar widget

- Shows the current performance mode and, optionally, CPU temperature.
- Click for a popup with:
  - **Mode**: Silent, Balanced, Turbo.
  - **GPU mode**: Integrated, Hybrid, Ultimate (confirmation required; applies after reboot).
  - **Quick toggles**: keyboard brightness, touchpad, lid-closed-awake on AC,
    panel overdrive, boot sound, music.
  - Battery and fan summary, and a shortcut to open the full window.
- Widget settings: dim the keyboard after N idle seconds, show or hide temperature.

### Window

- **Fans**: per-fan (CPU, GPU, mid) custom curves per mode. Drag points to edit,
  Shift-drag moves the whole curve, reset to firmware auto at any time.
- **CPU / GPU**: per-mode power tuning.
  - CPU: PL1 / PL2 power limits, CPU boost, energy preference (EPP), core + cache undervolt (when the BIOS allows it).
  - NVIDIA: Dynamic Boost, temperature target, core and memory clock offsets.
  - Live GPU status: clocks, temperature, power, load, VRAM, and what is keeping the dGPU awake.
- **Lighting**: keyboard brightness (remembered separately on AC and battery),
  effects with colours, speed and direction, and per-zone control including logo, lightbar
  and the light bar under the display (Lid).
- **Music effect** (per-key keyboards): pick *Music* in the effect list and the keyboard
  reacts to whatever is playing. Spectrum (bass to treble, left to right, bars rising
  with each band) or Pulse (everything follows loudness), in a gradient, rainbow or
  single colour, with a sensitivity setting. Fn+F4 cycles through it; it remembers
  on/off. `armoury light effect music`, `armoury music on|off|toggle|set …`.
- **Battery**: charge, health, cycles, voltage, draw and time left; charge limit
  with a one-shot "charge to 100%".
- **Input**: bind the ROG key, Fn+F5 and Fn+F4 (Aura) to open Armoury, cycle mode,
  cycle keyboard brightness, cycle lighting effect, toggle the NumberPad or music
  lighting, or run any command; touchpad toggle.
- **System**: panel refresh rate on AC and on battery, overdrive and gamma,
  performance mode on AC and on battery, sleep mode, lid-closed behaviour, boot sound.
- **Keystone**: actions when the Keystone goes in or out: performance mode,
  lighting (an effect, Music, or back to what was on before), a command, and lock
  the screen on remove. The Keystone light flashes on insert. One switch turns it all
  off. `armoury keystone [on|off]`.

### Daemon extras

- OSD for Fn+F2 / Fn+F3 keyboard brightness and the mode key.
- Stock modes send only the firmware policy change; Manual mode applies your own
  limits and fan curves, serialized to keep the embedded controller happy.

## Install

    ./install.sh

    armoury status          # what the daemon sees

## asusd lighting fix

asusd's model database lists the G533Z as keyboard-only, so the logo,
lightbar and the light bar under the display (Lid) never light. `armoury-root asusd-support-fix` adds the missing zones;
a pacman hook re-applies it after asusctl upgrades.

## Uninstall

    ./uninstall.sh
