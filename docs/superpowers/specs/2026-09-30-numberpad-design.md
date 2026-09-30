# NumberPad — design

Date: 2026-09-30 · Status: design agreed in chat, awaiting spec review
Extends: `2026-09-29-omarchy-armoury-design.md` §4.6

## Intent

- The illuminated NumberPad on the touchpad works under armouryd exactly as
  G-Helper's does today, so G-Helper can be retired.
- The user added these defaults, each changeable in settings:
  - it always turns on at maximum brightness
  - it doesn't work while the touchpad is disabled
  - it goes dark after a period without use

Measured on this laptop (G533ZW, 2026-09-30):
- The touchpad is `ASUE1403:00 04F3:319A Touchpad` (`/dev/input/event23` this
  boot), on i2c bus 4 (`/sys/.../i2c-4/i2c-ASUE1403:00`).
- G-Helper keeps the touchpad node open all the time. It takes it with
  `EVIOCGRAB` only while the NumberPad is on, so the cursor stops, and it
  types through its own uinput device, "g-helper numberpad".
- The cursor does not move while the NumberPad is on. The user wants that kept.

## 1. Where it runs

- A worker inside armouryd (`features/numpad.rs`), beside the hotkey reader.
  It uses the `evdev` crate (touchpad reading, grab, uinput virtual keyboard)
  and the `i2cdev` crate (backlight).
- It runs only in **active** mode. On handback, or when observe mode is
  entered, it turns off, releases the grab and switches the backlight off.
  G-Helper's own NumberPad then works as before.
- If armouryd dies, the kernel drops the grab with the file descriptor, so the
  touchpad can never stay stuck.

## 2. Hardware details

**Touchpad:** the first `/sys/class/input/event*` whose name matches
`ASUE* Touchpad`, `ASUF* Touchpad`, `ASUP* Touchpad` or `ELAN* Touchpad`,
looked up by name because event numbers change between boots. Its i2c bus is
the `i2c-N` ancestor of the matching `i2c-<ACPI name>` device in sysfs.

**Backlight** (from asus-numberpad-driver): one i2c write to address `0x15`
(`0x38` for ASUF1416, ASUF1205 and ASUF1204):

```
[0x05, 0x00, 0x3d, 0x03, 0x06, 0x00, 0x07, 0x00, 0x0d, 0x14, 0x03, V, 0xad]
```

- `V = 0x01` turns it on, `0x00` turns it off.
- Brightness levels 1–8 are `0x41`–`0x48`.

**Layout** (data table per model; `g533` is used for ASUE1403):
- 4 rows × 5 columns:
  - `7 8 9 / [icon]`
  - `4 5 6 * ⌫`
  - `1 2 3 - ↵`
  - `0 0 . + ↵`
- Margins from the touchpad edges, in touchpad units:
  - top 200, right 200, left 200, bottom 80
- Corner icons:
  - top-right icon: 1000 × 600
  - top-left icon: 250 × 250
- The key grid lies inside the margins:
  - `col = floor((x - minx) / col_width)`
  - `row = floor((y - miny) / row_height)`
  - the column count is the longest row; a row's missing cells are dead
- Keys are sent as keypad codes (`KEY_KP7` …, `KEY_KPSLASH`, `KEY_KPASTERISK`,
  `KEY_KPMINUS`, `KEY_KPPLUS`, `KEY_KPDOT`, `KEY_KPENTER`) plus `KEY_BACKSPACE`.

## 3. Behaviour

- **Always:** the touchpad is read (never grabbed while the NumberPad is off),
  to catch the activation hold.
- **Turning on:** a single finger held in the top-right icon for
  `hold_ms` (default 1000) toggles it. On turning on:
  - `EVIOCGRAB` the touchpad, so the cursor stops;
  - backlight on at `start_brightness` (default 8, the maximum);
  - if Hyprland reports NumLock off (`hyprctl devices -j`, main keyboard's
    `numLock`), send `KEY_NUMLOCK` once so keypad keys give digits.
- **While on:**
  - A touch that starts in a key cell presses that key (key down), and
    lifting releases it (key up). A touch that starts outside the grid, or in
    a dead cell, does nothing.
  - Only the first finger counts; extra fingers are ignored.
  - A tap in the top-left icon steps the brightness 1 → 8 → 1 for this
    session.
  - Holding the top-right icon for `hold_ms` turns it off.
- **Turning off:** release the grab, backlight off. NumLock is left as it is.
- **Touchpad off** (Omarchy's touchpad toggle; armouryd already reads its
  state file): unless `allow_when_touchpad_off` (default false),
  - the activation hold is ignored;
  - if the touchpad becomes disabled while the NumberPad is on, it turns off.
- **Idle:**
  - After `idle_dim_secs` (default 60; 0 = never) without a touch, the
    backlight turns off while the NumberPad stays on (still grabbed).
  - The next touch only wakes the backlight to the current brightness. It
    doesn't type a key, because the grid can't be seen while dark.
- **After a reboot** the NumberPad starts off.

## 4. Settings (`config.toml` `[numpad]`)

```toml
[numpad]
start_brightness = 8          # 1–8, level used every time it turns on
allow_when_touchpad_off = false
idle_dim_secs = 60            # 0 = never
hold_ms = 1000                # top-right hold to toggle (300–3000)
```

## 5. Requests, snapshot, UI

- **Requests:**
  - `set_numpad { on }`: refused in observe mode, and while the touchpad is
    off unless allowed.
  - `set_numpad_config { start_brightness?, allow_when_touchpad_off?,
    idle_dim_secs?, hold_ms? }`: validated and saved.
- **Snapshot:** `system.numpad`, one of `unavailable` (no supported
  touchpad), `off` or `on`.
- **Key action:** `toggle_numpad`, bindable to ROG / Fn+F4 / Fn+F5.
- **UI:**
  - The popup's quick-toggle grid gains a **NumberPad** tile.
  - The Input page gets a NumberPad section:
    - on/off
    - start brightness
    - "Allow while the touchpad is off"
    - idle timeout
    - hold time
- **Install:** `install.sh` adds udev rules (uaccess) for the touchpad event
  node, `/dev/i2c-*` and `/dev/uinput`, because G-Helper's rules disappear
  when it is uninstalled. `uninstall.sh` removes them.

## 6. Failures

- **Device missing:** touchpad or i2c node missing, or a read error
  (suspend/resume, driver reload): turn off, then reopen with the existing
  key-reader backoff.
- **Backlight write fails:** the NumberPad keeps working without light. The
  error appears in the snapshot's `apply_error`.
- **Worker crash loop:** more than 3 restarts in a minute and the worker
  stops, sets `system.numpad = "unavailable"`, and logs why.
- **No firmware writes:** the NumberPad never touches asusd or the EC (i2c to
  the touchpad controller only), so it's outside the EC-write rules.

## 7. Testing

**Pure unit tests:**
- hit-testing: every cell of the g533 table, margins, dead cells, corner icons
- hold detection
- touch state machine: key down/up pairing; first-finger-only; the waking
  touch types nothing
- idle timer
- touchpad-off rule
- packet bytes and address choice
- config validation

**Worker tests** with a fake event source and a fake virtual keyboard /
backlight:
- on, then type, then off
- the grab is released on observe mode
- device loss and reopen

**Hardware, in one takeover session ending in handback:**
1. Hold to turn on: cursor frozen, max brightness, NumLock handled.
2. Type `123.45+6↵` into a text field.
3. Top-left taps step the brightness.
4. Wait 60 s: goes dark; a touch wakes it without typing.
5. With Omarchy's touchpad off, the hold does nothing.
6. Hold to turn off: cursor works again.
7. Hand back; G-Helper's NumberPad still works.

## Out of scope

- A calculator launcher on the top-left icon.
- Swipe gestures.
- Layouts for other models beyond the table mechanism (added when needed).
