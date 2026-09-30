# Music-reactive lighting (Plan 9)

Expands spec §4.4. Agreed with the user 2026-09-30; they asked to go straight to
implementation after the in-chat design.

## Findings (on-device probe, 2026-09-30)

- asusd has no per-key interface here; frames go to the N-KEY hidraw node
  (`0b05:19b6`) as `0x5D 0xBC` feature reports, same as g-helper-linux
  `Aura.ApplyDirectZones`. Direct mode works on the G533ZW (g-helper marks
  only the G533Q broken). One frame = 11 key packets + 1 lightbar packet;
  the keyboard accepts ~45 fps.
- Re-applying the current effect through asusd (`set_mode_data`) leaves
  direct mode and restores normal lighting.
- `pw-record --raw --format f32 --rate 48000 --channels 1 -P '{ stream.capture.sink = true }' -`
  captures the default output (follows default-sink changes).
- USB HID only: no EC/ACPI calls, no polling.

## Behaviour

- Styles: **Spectrum** (18 log bands 40 Hz–16 kHz, one per key column, bars
  rise bottom-up) and **Pulse** (everything follows loudness).
- Colour schemes: **Gradient** (colour 1 → colour 2; by bar height in
  Spectrum, by loudness in Pulse), **Rainbow** (hue by column), **Single**
  (colour 1). Lightbar, logo and lid follow overall loudness.
- Automatic gain plus a sensitivity 1–10 (default 5); instant attack, ~0.5 s fall.
- Silence (≈1 s below −70 dBFS): keys off, one frame sent, then nothing until sound returns.
- 30 fps cap; unchanged frames are not sent.
- On/off persists; runs in active mode only; resumes on takeover/boot.
- Stop (toggle off, worker failure, handback): the worker stops sending,
  kills the capture child, and re-applies asusd's current effect.
- Setting or cycling an effect while music is on turns music off first.
- `kbd idle` pauses frames; `kbd resume` continues.
- Zones switched off for "awake" stay dark.
- Init packet re-sent every 5 s (covers suspend/resume and asusd rewrites).
- Handback: music released (stopped + effect restored + acked) before asusd stops.
- Capture child exit or HID write error = failure; more than 3 per minute →
  music disables itself with the error shown.

## Structure

`crates/armouryd/src/features/music/`: `perkey.rs` (LED table, packet
builder: the reusable per-key frame layer), `analyze.rs` (FFT, bands, AGC,
smoothing, silence), `render.rs` (analysis → frame), `worker.rs` (loop,
capture child, hidraw IO, restore). Config `[music]`; requests `SetMusic`,
`SetMusicConfig`; snapshot `lighting.music` + `lighting.music_error`;
key action `toggle_music`; CLI `armoury music`; popup toggle + Lighting
page section. udev `uaccess` rule for the N-KEY hidraw node.
