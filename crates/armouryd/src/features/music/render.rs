//! Analysis → per-key frame for the chosen style and colour scheme.
use super::analyze::{Analysis, BANDS};
use super::perkey::{Frame, Layout, Place, Rgb};
use crate::config::MusicConfig;
use armoury_proto::{AuraZone, MusicScheme, MusicStyle};

/// Which lighting zones are on while awake (keyboard, lightbar, logo); the rest stay dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lit { pub keyboard: bool, pub lightbar: bool, pub logo: bool }

impl Default for Lit {
    fn default() -> Self { Self { keyboard: true, lightbar: true, logo: true } }
}

impl Lit {
    /// From asusd's zone power: a zone is lit if it is on while awake (absent zones count as on).
    pub fn from_zones(zones: &[armoury_proto::ZonePower]) -> Self {
        let awake = |z| zones.iter().find(|p| p.zone == z).is_none_or(|p| p.awake);
        Self { keyboard: awake(AuraZone::Keyboard), lightbar: awake(AuraZone::Lightbar), logo: awake(AuraZone::Logo) }
    }

    fn led(&self, idx: u8) -> bool {
        match idx {
            169..=174 => self.lightbar,
            167 | 176 | 177 => self.logo,
            _ => self.keyboard,
        }
    }
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

fn scale(c: Rgb, k: f32) -> Rgb { c.map(|v| (v as f32 * k.clamp(0.0, 1.0)).round() as u8) }

/// Full-saturation colour; h 0..1 around the wheel.
fn hue(h: f32) -> Rgb {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = h6 - h6.floor();
    let (q, t) = ((255.0 * (1.0 - f)).round() as u8, (255.0 * f).round() as u8);
    match h6 as u8 { 0 => [255, t, 0], 1 => [q, 255, 0], 2 => [0, 255, t], 3 => [0, q, 255], 4 => [t, 0, 255], _ => [255, 0, q] }
}

/// Bass (0) is red, treble (1) violet.
fn rainbow(x: f32) -> Rgb { hue(x.clamp(0.0, 1.0) * 0.8) }

pub fn render(layout: &Layout, a: &Analysis, cfg: &MusicConfig, lit: Lit) -> Frame {
    let mut f = Frame::default();
    let (c1, c2) = (cfg.colour1, cfg.colour2);
    let top = (layout.rows.max(2) - 1) as f32;
    for led in &layout.leds {
        if !lit.led(led.idx) { continue; }
        let colour = match (led.place, cfg.style) {
            (Place::Key { x, row }, MusicStyle::Spectrum) => {
                let band = ((x * BANDS as f32) as usize).min(BANDS - 1);
                let height = top - row as f32; // 0 = bottom row
                let on = (a.bands[band] * layout.rows as f32 - height).clamp(0.0, 1.0);
                let c = match cfg.scheme {
                    MusicScheme::Gradient => lerp(c1, c2, height / top),
                    MusicScheme::Rainbow => rainbow(band as f32 / (BANDS - 1) as f32),
                    MusicScheme::Single => c1,
                };
                scale(c, on)
            }
            (Place::Key { x, .. }, MusicStyle::Pulse) => {
                let c = match cfg.scheme {
                    MusicScheme::Gradient => lerp(c1, c2, a.loudness),
                    MusicScheme::Rainbow => rainbow(x),
                    MusicScheme::Single => c1,
                };
                scale(c, a.loudness)
            }
            (Place::Ambient, _) => {
                let c = match cfg.scheme {
                    MusicScheme::Gradient => lerp(c1, c2, a.loudness),
                    MusicScheme::Rainbow => rainbow(a.loudness),
                    MusicScheme::Single => c1,
                };
                scale(c, a.loudness)
            }
        };
        f.0[led.idx as usize] = colour;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::perkey::layout_for;

    fn g533() -> Layout { layout_for("G533ZW").unwrap() }
    fn analysis(bands: [f32; BANDS], loudness: f32) -> Analysis { Analysis { bands, loudness, silent: false } }
    fn cfg(style: MusicStyle, scheme: MusicScheme) -> MusicConfig {
        MusicConfig { style, scheme, colour1: [0, 0, 255], colour2: [255, 0, 0], ..MusicConfig::default() }
    }
    const ESC: usize = 21;
    const LCTRL: usize = 126;
    const HOME: usize = 41;
    const LOGO: usize = 167;
    const LIGHTBAR: usize = 170;

    #[test]
    fn spectrum_bars_rise_from_the_bottom() {
        let mut b = [0.0; BANDS];
        b[0] = 0.5; // left band half full
        let f = render(&g533(), &analysis(b, 0.0), &cfg(MusicStyle::Spectrum, MusicScheme::Single), Lit::default());
        assert_eq!(f.0[LCTRL], [0, 0, 255], "bottom-left key lit");
        assert_eq!(f.0[ESC], [0, 0, 0], "top of a half bar is dark");
        assert_eq!(f.0[HOME], [0, 0, 0], "other bands dark");
        let full = render(&g533(), &analysis([1.0; BANDS], 0.0), &cfg(MusicStyle::Spectrum, MusicScheme::Single), Lit::default());
        assert!([ESC, LCTRL, HOME].iter().all(|i| full.0[*i] == [0, 0, 255]));
    }

    #[test]
    fn gradient_runs_bottom_to_top_and_rainbow_left_to_right() {
        let full = analysis([1.0; BANDS], 1.0);
        let g = render(&g533(), &full, &cfg(MusicStyle::Spectrum, MusicScheme::Gradient), Lit::default());
        assert_eq!(g.0[LCTRL], [0, 0, 255], "bottom = colour 1");
        assert_eq!(g.0[2], [255, 0, 0], "top (media row) = colour 2");
        let r = render(&g533(), &full, &cfg(MusicStyle::Spectrum, MusicScheme::Rainbow), Lit::default());
        assert_eq!(r.0[ESC], [255, 0, 0], "bass is red");
        assert!(r.0[HOME][2] > 200, "treble is violet: {:?}", r.0[HOME]);
    }

    #[test]
    fn pulse_follows_loudness_everywhere() {
        let c = cfg(MusicStyle::Pulse, MusicScheme::Single);
        let half = render(&g533(), &analysis([0.0; BANDS], 0.5), &c, Lit::default());
        assert!([ESC, LCTRL, HOME, LOGO, LIGHTBAR].iter().all(|i| half.0[*i] == [0, 0, 128]), "{:?}", half.0[ESC]);
        let quiet = render(&g533(), &analysis([1.0; BANDS], 0.0), &c, Lit::default());
        assert!(quiet.0.iter().all(|c| *c == [0, 0, 0]));
    }

    #[test]
    fn ambient_lights_follow_loudness_in_spectrum() {
        let f = render(&g533(), &analysis([0.0; BANDS], 1.0), &cfg(MusicStyle::Spectrum, MusicScheme::Single), Lit::default());
        assert_eq!((f.0[LOGO], f.0[LIGHTBAR], f.0[ESC]), ([0, 0, 255], [0, 0, 255], [0, 0, 0]));
    }

    #[test]
    fn zones_switched_off_stay_dark() {
        let lit = Lit { keyboard: true, lightbar: false, logo: false };
        let f = render(&g533(), &analysis([1.0; BANDS], 1.0), &cfg(MusicStyle::Pulse, MusicScheme::Single), lit);
        assert_eq!((f.0[LOGO], f.0[LIGHTBAR], f.0[176]), ([0, 0, 0], [0, 0, 0], [0, 0, 0]));
        assert_eq!(f.0[ESC], [0, 0, 255]);
    }

    #[test]
    fn lit_from_zone_power() {
        let z = |zone, awake| armoury_proto::ZonePower { zone, boot: true, awake, sleep: true, shutdown: true };
        assert_eq!(Lit::from_zones(&[z(AuraZone::Keyboard, true), z(AuraZone::Lightbar, false)]),
                   Lit { keyboard: true, lightbar: false, logo: true });
    }

    #[test]
    fn colour_helpers() {
        assert_eq!(lerp([0, 0, 0], [255, 255, 255], 0.5), [128, 128, 128]);
        assert_eq!((hue(0.0), hue(1.0 / 3.0), hue(2.0 / 3.0)), ([255, 0, 0], [0, 255, 0], [0, 0, 255]));
    }
}
