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

## Keystone LED — for Plan 10, 2026-09-30

The Keystone LED is direct-mode LED 175 (slot 8 of the lightbar/logo packet), confirmed on
the G533ZW. g-helper's "KSTN" LED 0 lights nothing. Music lighting drives 175 with the
ambient lights. Plan 10 can use it, e.g. to flash on insert/remove.

## Display light bar independent of F5/Delete — 2026-10-01

In per-key (0x5D 0xBC) mode, the bar under the display takes F5's (28) and Delete's (37)
colours, and only when the lid power zone is on. LEDs 176/177 don't drive it. OpenRGB's
G533ZW driver does the same. Windows Armoury Crate drives it independently, so there is
another packet. Waiting on a USBPcap capture from the user's Windows. The keyboard only
has vendor reports (0x5D, 0x5A, 0xA5, 0xC1, 0xC2); don't blind-probe them.

## Other deferred items

- Software Fn-lock: the user doesn't use it. There is no firmware attribute
  on the G533ZW; it would need a keyboard grab plus uinput.
- Automated Integrated↔Ultimate GPU switching (manual two-step via Hybrid for
  now) and real GPU-switch testing (reboots; needs the user).
- Window layout options for the Armoury window.
- Per-key lighting editor.
