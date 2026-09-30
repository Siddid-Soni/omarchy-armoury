//! NumberPad key grids (per model) and the touchpad backlight packet
//! (from asus-numberpad-driver; verified against G-Helper on the G533ZW).

pub const LIGHT_ON: u8 = 0x01;
pub const LIGHT_OFF: u8 = 0x00;
pub const KEY_NUMLOCK: u16 = 69;

/// Brightness level 1–8 → packet value. The pad uses the low 4 bits (16 steps,
/// asus-numberpad-driver issue #109), so the 8 levels are spread over 0x42..=0x4F
/// and level 8 is full brightness (0x41..0x48 only reached half).
pub fn level_byte(level: u8) -> u8 { 0x40 + ((level.clamp(1, 8) as u16 * 15 + 4) / 8) as u8 }

pub fn backlight_packet(v: u8) -> [u8; 13] {
    [0x05, 0x00, 0x3d, 0x03, 0x06, 0x00, 0x07, 0x00, 0x0d, 0x14, 0x03, v, 0xad]
}

pub fn i2c_address(touchpad_name: &str) -> u16 {
    if ["ASUF1416", "ASUF1205", "ASUF1204"].iter().any(|p| touchpad_name.starts_with(p)) { 0x38 } else { 0x15 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area { pub minx: i32, pub maxx: i32, pub miny: i32, pub maxy: i32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit { Key(u16), RightIcon, LeftIcon, None }

pub struct Layout {
    pub name: &'static str,
    /// Touchpad name prefixes this layout is for.
    pub touchpads: &'static [&'static str],
    /// Rows of evdev key codes; shorter rows leave dead cells on the right.
    pub keys: &'static [&'static [u16]],
    pub top: i32, pub right: i32, pub left: i32, pub bottom: i32,
    pub right_icon: (i32, i32),
    pub left_icon: (i32, i32),
}

impl Layout {
    pub fn hit(&self, a: &Area, x: i32, y: i32) -> Hit {
        if x >= a.maxx - self.right_icon.0 && y <= a.miny + self.right_icon.1 { return Hit::RightIcon; }
        if x <= a.minx + self.left_icon.0 && y <= a.miny + self.left_icon.1 { return Hit::LeftIcon; }
        let (x0, x1, y0, y1) = (a.minx + self.left, a.maxx - self.right, a.miny + self.top, a.maxy - self.bottom);
        if x < x0 || x >= x1 || y < y0 || y >= y1 { return Hit::None; }
        let cols = self.keys.iter().map(|r| r.len()).max().unwrap_or(1) as i32;
        let rows = self.keys.len() as i32;
        let col = ((x - x0) * cols / (x1 - x0)) as usize;
        let row = ((y - y0) * rows / (y1 - y0)) as usize;
        self.keys.get(row).and_then(|r| r.get(col)).map_or(Hit::None, |&k| Hit::Key(k))
    }
}

/// ROG Strix G533 (and G614JVR): 5 columns, NumLock icon top right.
pub const G533: Layout = Layout {
    name: "g533",
    touchpads: &["ASUE1403"],
    keys: &[&[71, 72, 73, 98], &[75, 76, 77, 55, 14], &[79, 80, 81, 74, 96], &[82, 82, 83, 78, 96]],
    top: 200, right: 200, left: 200, bottom: 80,
    right_icon: (1000, 600),
    left_icon: (250, 250),
};

const LAYOUTS: &[&Layout] = &[&G533];

pub fn layout_for(touchpad_name: &str) -> Option<&'static Layout> {
    LAYOUTS.iter().copied().find(|l| l.touchpads.iter().any(|p| touchpad_name.starts_with(p)))
}
#[cfg(test)]
mod tests {
    use super::*;
    // this laptop's touchpad: X 0..4036, Y 0..2299
    const A: Area = Area { minx: 0, maxx: 4036, miny: 0, maxy: 2299 };
    // centre of cell (col,row) inside the g533 margins
    fn cell(col: i32, row: i32) -> (i32, i32) {
        let (w, h) = ((4036 - 200 - 200) as f32 / 5.0, (2299 - 200 - 80) as f32 / 4.0);
        ((200.0 + w * (col as f32 + 0.5)) as i32, (200.0 + h * (row as f32 + 0.5)) as i32)
    }

    #[test]
    fn every_g533_cell() {
        let want = [[71, 72, 73, 98, 0], [75, 76, 77, 55, 14], [79, 80, 81, 74, 96], [82, 82, 83, 78, 96]];
        for (row, keys) in want.iter().enumerate() {
            for (col, &k) in keys.iter().enumerate() {
                let (x, y) = cell(col as i32, row as i32);
                let got = G533.hit(&A, x, y);
                let expect = if k == 0 { Hit::None } else { Hit::Key(k) };
                if row == 0 && col == 4 { assert_eq!(got, Hit::RightIcon, "row 0 col 4 is the icon"); } else { assert_eq!(got, expect, "row {row} col {col}"); }
            }
        }
    }

    #[test]
    fn margins_and_icons() {
        assert_eq!(G533.hit(&A, 100, 1200), Hit::None, "left margin");
        assert_eq!(G533.hit(&A, 2000, 2250), Hit::None, "bottom margin");
        assert_eq!(G533.hit(&A, 4000, 50), Hit::RightIcon);
        assert_eq!(G533.hit(&A, 100, 100), Hit::LeftIcon);
        assert!(layout_for("ASUE1403:00 04F3:319A Touchpad").is_some());
        assert!(layout_for("SYNA1234 Touchpad").is_none());
    }

    #[test]
    fn packet_and_address() {
        assert_eq!(backlight_packet(LIGHT_ON), [0x05, 0x00, 0x3d, 0x03, 0x06, 0x00, 0x07, 0x00, 0x0d, 0x14, 0x03, 0x01, 0xad]);
        // the pad uses the low 4 bits (16 steps; asus-numberpad-driver issue #109): level 8 is 0x4F, full brightness
        assert_eq!(backlight_packet(level_byte(8))[11], 0x4F);
        let bytes: Vec<u8> = (1..=8).map(level_byte).collect();
        assert!(bytes.windows(2).all(|w| w[0] < w[1]) && bytes[0] > 0x40, "8 distinct rising levels, none off: {bytes:x?}");
        assert_eq!(i2c_address("ASUE1403:00 04F3:319A Touchpad"), 0x15);
        assert_eq!(i2c_address("ASUF1416:00 2808:0108 Touchpad"), 0x38);
    }
}
