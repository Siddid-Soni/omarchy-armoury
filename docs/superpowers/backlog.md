# Backlog (deferred by the user)

Items the user asked to keep for later. Each gets its own spec/plan when picked up.

## Key rebinding (M1–M4, Fn+F1…F12) — non-priority, 2026-09-30

G-Helper lets every hotkey be rebound. On the G533ZW, M1–M4 are the top-row
volume down / volume up / mic mute / ROG keys; M5 is Fn+F5.

Proposed approach (not yet approved):
- Keys that send normal key codes Hyprland sees (volume, brightness, mic):
  a rebound key has its Omarchy bind replaced in the generated
  `~/.config/hypr/omarchy-armoury.lua` (the file that already holds SUPER+W),
  pointing at `armoury key <id>`. "Default" keeps Omarchy's bind.
- Keys Hyprland can't see (ROG, Fn+F5): armouryd's existing evdev reader.
- Rejected alternative: exclusive grab of the keyboard devices plus uinput
  passthrough. It's invasive, and the keys die with the daemon.
- Actions:
  - Default, Nothing
  - Open Armoury, Cycle mode, Manual on/off
  - Keyboard brightness, Lighting effect
  - Toggle touchpad, Mic mute, Screenshot
  - Run command
- UI: the Input page lists every captured key with a dropdown.
- First step: a key-capture session with the user to list the exact codes
  each key sends.

## Keystone LED in music lighting — for Plan 10, 2026-09-30

The light bar under the display is the Keystone LED. It stays dark while music lighting
is on: direct-mode frames don't drive it. We send loudness to LED 0 (g-helper's "KSTN"),
and lighting-packet slots 8–17 (LEDs 175+) had no effect either. It does light with
normal asusd effects. Find out how to drive it (another report or zone), or leave it on
its effect while music runs.

## Other deferred items

- Software Fn-lock: the user doesn't use it. There is no firmware attribute
  on the G533ZW; it would need a keyboard grab plus uinput.
- Automated Integrated↔Ultimate GPU switching (manual two-step via Hybrid for
  now) and real GPU-switch testing (reboots; needs the user).
- Window layout options for the Armoury window.
- Per-key lighting editor.
