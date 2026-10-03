# Mapping the G533ZW light bars with USBPcap (Windows)

On Linux, the bar under the display mirrors F5 (LED 28) and Delete (LED 37), and only while
the lid power zone is on. Armoury Crate on Windows drives it independently, so it sends a
packet we don't know yet. These steps capture what Armoury Crate sends to the keyboard, so
we can decode the packet on Linux. Nothing here sends anything to the keyboard: USBPcap
only listens.

## What we know already

| Thing | Value |
|---|---|
| Keyboard USB device | `0b05:19b6` "N-KEY Device" |
| Direct (per-key) mode | report `0x5D 0xBC`, 64-byte packets, 16 LEDs each, 178 LEDs |
| Front light bar | LEDs 174 → 169, left to right |
| Logo | LED 167 |
| Keystone LED | LED 175 |
| Lid "vertical cut" | LEDs 176, 177: **do nothing** on Linux |
| Display bar | no LED index of its own; copies F5 (28) and Delete (37) |
| Zone power | `0x5D 0xBD 01 keyb bar lid rear FF` |
| Other vendor reports | `0x5A`, `0xA5`, `0xC1`, `0xC2`: never probed, so we don't know what they do |

## 1. Install (once)

1. Install **Wireshark** from wireshark.org. In the installer, tick **Install USBPcap**.
2. Reboot. USBPcap needs it.
3. Make a folder **`C:\armoury-captures`**. Linux can read the Windows partition from there.

## 2. Find the keyboard's capture interface

1. Run Wireshark **as Administrator**.
2. The interface list shows `USBPcap1`, `USBPcap2` and so on. Click the **gear** next to
   each one until a device list shows **N-KEY Device** (or `0b05:19b6`).
3. On that interface:
   - Untick everything except the N-KEY device. That keeps the capture small.
   - Tick **Inject already connected devices descriptors**. Wireshark then decodes HID.
   - Turn off **Capture from newly connected devices**.
4. Start it and type a few keys. If packets appear, that's the right interface. Stop it again.

## 3. Rules for every capture

- **Close other RGB software**: OpenRGB, SignalRGB, iCUE and Aura Sync to other devices.
  Armoury Crate must be the only thing talking to the keyboard.
- **Markers:** before each step, **tap Left Ctrl as many times as the step number**. Key
  presses come from the same USB device, so they show up in the capture and separate the steps.
- **Wait about 5 seconds** after each change before starting the next one.
- Press **Apply/Save** whenever Armoury Crate or Aura Creator asks for it.
- Note your **Armoury Crate and Aura Creator versions** (Settings → About).
- Don't touch keyboard firmware updates during a capture.

## 4. The captures

Use one Wireshark capture per file: Start → do the steps → Stop → **File → Save As**
→ `pcapng` into `C:\armoury-captures\`.

### `01-static-zones.pcapng`: one zone at a time

Use **Aura Creator**, which lets you pick individual LEDs and bars. Use a **Static** layer for
all of these steps.

| Step | Set |
|---|---|
| 1 | everything **white** `FFFFFF` |
| 2 | everything **off/black**, display bar only **red** `FF0000` |
| 3 | display bar only **green** `00FF00` |
| 4 | display bar only **blue** `0000FF` |
| 5 | display bar **off**, **F5 only** green |
| 6 | **Delete only** green |
| 7 | **front light bar only** red |
| 8 | **logo only** red (skip if Aura Creator doesn't list it) |
| 9 | **lid / vertical-cut lights only** red (if listed separately from the display bar) |
| 10 | **Keystone LED only** red (if listed) |
| 11 | display bar **red**, F5 and Delete **blue** at the same time |

Step 11 is the important one: it proves whether the bar can differ from F5 and Delete.

### `02-zone-power.pcapng`: power and lighting toggles

In **Armoury Crate → Device → Lighting → Lighting settings** (the per-zone toggles for
Boot / Awake / Sleep / Shutdown), toggle each one off, wait, then on again. Use one Ctrl
marker per toggle and write the order down in `notes.txt`. Cover at least: **keyboard
awake, light bar awake, lid awake, rear awake**.

### `03-bar-effect.pcapng`: optional, an animated effect on the display bar

Set a **Color cycle** or **Breathing** effect on the **display bar only**, and leave it for
about 10 seconds. This shows whether the animation runs in firmware (a few packets) or is
streamed by Armoury Crate (constant packets).

### `notes.txt`

Write down what you did in each step, anything that looked different from the plan, and
the versions. Short is fine.

## 5. Back on Linux

```sh
sudo mount -o ro /dev/nvme0n1p3 /mnt   # Windows "OS" partition, read-only
ls /mnt/armoury-captures
```

If the mount refuses because Windows is hibernated or fast-started, boot Windows again and
**hold Shift while clicking Shut down**. Then give Claude the path. The decode looks at
`SET_REPORT` (bRequest 9) feature reports and interrupt OUT transfers to `0b05:19b6`. Then
the bar becomes its own light in the per-key layer and music, and the F5/Delete mirroring goes.

## While you're in Windows: Keystone write test

The Keystone is read over the **NFC reader on i2c**, not USB, so USBPcap **can't see** that
traffic. We check it by comparing the tag before and after instead. The baseline is in
`tools/keystone-nfc/baseline/`: block 0 = `01 01 01 01` (locked), blocks 1–79 all zero.

1. With the Keystone inserted, do everything Armoury Crate offers for it: bind and unbind,
   insert and remove actions, Shadow Drive setup or unlock, renaming, and so on.
2. In `notes.txt`, write down **what you did, in order**.
3. Back on Linux, run:

   ```sh
   cd tools/keystone-nfc && ./build.sh && pkexec ./verify.sh after-windows
   ```

   `UNCHANGED` means Armoury Crate wrote nothing to the tag. `CHANGED` prints a diff of
   blocks, locks and settings.
