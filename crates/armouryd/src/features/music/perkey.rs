//! Per-key frame layer: where each LED sits on the laptop and the direct-mode HID packets
//! (`0x5D 0xBC`) that paint a whole frame. Protocol and LED indices from g-helper-linux
//! `Aura.ApplyDirectZones` (per-key Strix). Used by music lighting; kept independent of it
//! so a per-key editor can reuse it.

pub const LEDS: usize = 178;
/// LEDs 0..KEYSET go in the key packets, the rest (lightbar, logo, lid) in one extra packet.
const KEYSET: usize = 167;
const PER_PACKET: usize = 16;
pub const REPORT_ID: u8 = 0x5D;
pub const PACKET_LEN: usize = 64;

pub type Rgb = [u8; 3];

/// One frame: a colour per LED, indexed by the LED's packet index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame(pub [Rgb; LEDS]);

impl Default for Frame {
    fn default() -> Self { Self([[0; 3]; LEDS]) }
}

/// Where an LED is: a key at (x 0..1 left→right, row 0 = top), or an ambient light
/// (lightbar, logo, lid, Keystone) that has no place on the key grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    Key { x: f32, row: u8 },
    Ambient,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Led {
    pub idx: u8,
    pub place: Place,
}

pub struct Layout {
    pub leds: Vec<Led>,
    /// Key rows, top (0) to bottom (rows - 1).
    pub rows: u8,
}

/// Enters direct mode. Sent before the first frame and again now and then (the firmware
/// drops direct mode on resume and when asusd writes an effect).
pub fn init_packet() -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[..3].copy_from_slice(&[REPORT_ID, 0xBC, 1]);
    p
}

/// The feature reports that paint `frame`: 11 key packets of up to 16 LEDs, then the
/// lightbar/logo/lid packet.
pub fn packets(frame: &Frame) -> Vec<[u8; PACKET_LEN]> {
    let mut out = Vec::with_capacity(KEYSET.div_ceil(PER_PACKET) + 1);
    for start in (0..KEYSET).step_by(PER_PACKET) {
        let n = PER_PACKET.min(KEYSET - start);
        let mut p = [0u8; PACKET_LEN];
        p[..9].copy_from_slice(&[REPORT_ID, 0xBC, 0, 1, 1, 1, start as u8, n as u8, 0]);
        for (i, c) in frame.0[start..start + n].iter().enumerate() { p[9 + 3 * i..12 + 3 * i].copy_from_slice(c); }
        out.push(p);
    }
    let mut p = [0u8; PACKET_LEN];
    p[..9].copy_from_slice(&[REPORT_ID, 0xBC, 0, 1, 4, 0, 0, 0, 0]);
    for (i, c) in frame.0[KEYSET..].iter().enumerate() { p[9 + 3 * i..12 + 3 * i].copy_from_slice(c); }
    out.push(p);
    out
}

/// A key row: (LED index, width in key units), left to right.
type Row = &'static [(u8, f32)];

/// ROG Strix Scar 15 (G533): the g-helper per-key map without the 17" numpad LEDs.
/// Four LEDs g-helper leaves unnamed (120, 140, 141, 143) are placed by the arrow keys.
const G533_ROWS: [Row; 7] = [
    // Vol-, Vol+, mic mute, fan, Armoury Crate: above F1–F5 (placed on the F-row's grid below)
    &[(2, 1.0), (3, 1.0), (4, 1.0), (5, 1.0), (6, 1.0)],
    // Esc, F1–F12, Del, Del (17"), Pause, PrtSc, Home
    &[(21, 1.0), (23, 1.0), (24, 1.0), (25, 1.0), (26, 1.0), (28, 1.0), (29, 1.0), (30, 1.0), (31, 1.0),
      (33, 1.0), (34, 1.0), (35, 1.0), (36, 1.0), (37, 1.0), (38, 1.0), (39, 1.0), (40, 1.0), (41, 1.0)],
    // ` 1–0 - = Backspace×3, Play
    &[(42, 1.0), (43, 1.0), (44, 1.0), (45, 1.0), (46, 1.0), (47, 1.0), (48, 1.0), (49, 1.0), (50, 1.0),
      (51, 1.0), (52, 1.0), (53, 1.0), (54, 1.0), (55, 0.7), (56, 0.7), (57, 0.7), (58, 1.0)],
    // Tab Q–] \ Stop
    &[(63, 1.5), (64, 1.0), (65, 1.0), (66, 1.0), (67, 1.0), (68, 1.0), (69, 1.0), (70, 1.0), (71, 1.0),
      (72, 1.0), (73, 1.0), (74, 1.0), (75, 1.0), (76, 1.5), (79, 1.0)],
    // Caps A–# Enter×3, Previous
    &[(84, 1.75), (85, 1.0), (86, 1.0), (87, 1.0), (88, 1.0), (89, 1.0), (90, 1.0), (91, 1.0), (92, 1.0),
      (93, 1.0), (94, 1.0), (95, 1.0), (96, 1.0), (97, 0.75), (98, 0.75), (99, 0.75), (100, 1.0)],
    // LShift, ISO \, Z–/, RShift×3, (120), Up, Next
    &[(105, 1.25), (106, 1.0), (107, 1.0), (108, 1.0), (109, 1.0), (110, 1.0), (111, 1.0), (112, 1.0),
      (113, 1.0), (114, 1.0), (115, 1.0), (116, 1.0), (117, 0.6), (118, 0.6), (119, 0.6), (120, 0.5), (139, 1.0), (121, 1.0)],
    // Ctrl Fn Win Alt Space Alt Fn Ctrl, Left Down Right, (140 141 143), PrtSc
    &[(126, 1.25), (127, 1.0), (128, 1.0), (129, 1.25), (131, 6.0), (135, 1.0), (136, 1.0), (137, 1.0),
      (159, 1.0), (160, 1.0), (161, 1.0), (140, 0.3), (141, 0.3), (143, 0.3), (142, 1.0)],
];
/// Lightbar (left→right), Keystone, logo, lid left/right.
const G533_AMBIENT: &[u8] = &[174, 173, 172, 171, 170, 169, 0, 167, 176, 177];

fn build(rows: &[Row], ambient: &[u8]) -> Layout {
    let mut leds = Vec::new();
    // the media row sits over F1–F5: lay it on the F-row's grid, shifted one key right
    let f_row: f32 = rows[1].iter().map(|(_, w)| w).sum();
    for (r, row) in rows.iter().enumerate() {
        let (total, mut at) = if r == 0 { (f_row, 1.0) } else { (row.iter().map(|(_, w)| w).sum(), 0.0) };
        for &(idx, w) in *row {
            leds.push(Led { idx, place: Place::Key { x: (at + w / 2.0) / total, row: r as u8 } });
            at += w;
        }
    }
    leds.extend(ambient.iter().map(|&idx| Led { idx, place: Place::Ambient }));
    Layout { leds, rows: rows.len() as u8 }
}

/// The per-key layout for a model (DMI product name), if it has one.
pub fn layout_for(product: &str) -> Option<Layout> {
    product.contains("G533").then(|| build(&G533_ROWS, G533_AMBIENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_packet_enters_direct_mode() {
        assert_eq!(init_packet()[..4], [0x5D, 0xBC, 1, 0]);
    }

    #[test]
    fn packets_match_g_helper_per_key_layout() {
        let mut f = Frame::default();
        f.0[21] = [1, 2, 3]; // Esc
        f.0[166] = [4, 5, 6]; // last key LED
        f.0[167] = [7, 8, 9]; // logo
        f.0[177] = [10, 11, 12]; // lid right
        let p = packets(&f);
        assert_eq!(p.len(), 12, "11 key packets + 1 lightbar packet");
        assert_eq!(p[0][..9], [0x5D, 0xBC, 0, 1, 1, 1, 0, 16, 0]);
        assert_eq!(p[1][..9], [0x5D, 0xBC, 0, 1, 1, 1, 16, 16, 0]);
        assert_eq!(p[1][9 + 3 * 5..12 + 3 * 5], [1, 2, 3], "Esc = LED 21 = packet 1, slot 5");
        assert_eq!(p[10][..9], [0x5D, 0xBC, 0, 1, 1, 1, 160, 7, 0], "last key packet carries 7 LEDs");
        assert_eq!(p[10][9 + 3 * 6..12 + 3 * 6], [4, 5, 6]);
        assert!(p[10][12 + 3 * 6..].iter().all(|b| *b == 0), "nothing past the 7th LED");
        assert_eq!(p[11][..9], [0x5D, 0xBC, 0, 1, 4, 0, 0, 0, 0]);
        assert_eq!(p[11][9..12], [7, 8, 9]);
        assert_eq!(p[11][9 + 30..12 + 30], [10, 11, 12], "LED 177 = 11th slot of the lightbar packet");
    }

    #[test]
    fn g533_layout_is_sane() {
        let l = layout_for("ROG Strix G533ZW_G533ZW").unwrap();
        assert_eq!(l.rows, 7);
        let mut idx: Vec<u8> = l.leds.iter().map(|l| l.idx).collect();
        idx.sort();
        let n = idx.len();
        idx.dedup();
        assert_eq!(idx.len(), n, "no LED listed twice");
        assert!(idx.iter().all(|i| (*i as usize) < LEDS));
        for led in &l.leds {
            if let Place::Key { x, row } = led.place {
                assert!((0.0..=1.0).contains(&x) && row < 7, "{led:?}");
            }
        }
        let x = |i: u8| match l.leds.iter().find(|l| l.idx == i).unwrap().place { Place::Key { x, .. } => x, _ => panic!() };
        assert!(x(21) < 0.05 && x(41) > 0.95, "Esc far left, Home far right");
        assert!((x(131) - 0.4).abs() < 0.1, "space bar near the middle");
        assert!(x(2) > x(21) && x(6) < 0.4, "media keys over F1–F5");
        assert!(layout_for("ROG Zephyrus G14").is_none());
    }
}
