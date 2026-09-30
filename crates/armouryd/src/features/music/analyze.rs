//! Audio → band levels and loudness (0..1), with automatic gain, smoothing and silence
//! detection. Pure: fed chunks of mono f32 samples, one chunk per frame.
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

pub const RATE: usize = 48_000;
/// Samples per frame: 30 frames per second.
pub const CHUNK: usize = RATE / 30;
const FFT: usize = 2048;
pub const BANDS: usize = 18;
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
/// A level drops at most this much per frame (full scale in half a second).
pub const FALL: f32 = 1.0 / 15.0;
/// The gain reference falls 3 dB per second when the music gets quieter.
const AGC_DECAY_DB: f32 = 0.1;
/// Never amplify below this (so near-silence doesn't light everything).
const AGC_FLOOR_DB: f32 = -50.0;
const SILENCE_DB: f32 = -70.0;
/// Frames below SILENCE_DB before it counts as silence (1 s).
const SILENT_AFTER: usize = 30;

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub bands: [f32; BANDS],
    pub loudness: f32,
    pub silent: bool,
}

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// The last FFT samples, oldest first.
    history: Vec<f32>,
    /// FFT bin range per band.
    bins: Vec<(usize, usize)>,
    /// Per-band tilt (dB): +3 dB per octave around 1 kHz, so treble shows as well as bass.
    tilt: Vec<f32>,
    agc_db: f32,
    loud_agc_db: f32,
    levels: [f32; BANDS],
    loudness: f32,
    quiet_frames: usize,
}

fn db(x: f32) -> f32 { 20.0 * (x.max(1e-9)).log10() }

/// Level 0..1 of `v_db` against a gain reference: the top `range_db` below it maps to 0..1.
fn level(v_db: f32, ref_db: f32, range_db: f32) -> f32 { ((v_db - (ref_db - range_db)) / range_db).clamp(0.0, 1.0) }

impl Default for Analyzer {
    fn default() -> Self { Self::new() }
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT);
        let window = (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT - 1) as f32).cos()).collect();
        let bin_hz = RATE as f32 / FFT as f32;
        let edge = |k: usize| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(k as f32 / BANDS as f32);
        let bins = (0..BANDS).map(|k| {
            let lo = (edge(k) / bin_hz).floor() as usize;
            (lo, ((edge(k + 1) / bin_hz).floor() as usize).max(lo + 1))
        }).collect();
        let tilt = (0..BANDS).map(|k| 3.0 * ((edge(k) * edge(k + 1)).sqrt() / 1000.0).log2()).collect();
        Self { fft, window, history: vec![0.0; FFT], bins, tilt, agc_db: AGC_FLOOR_DB, loud_agc_db: AGC_FLOOR_DB,
               levels: [0.0; BANDS], loudness: 0.0, quiet_frames: 0 }
    }

    /// One frame's worth of samples. `sensitivity` 1..=10 widens the range shown below the peak.
    pub fn feed(&mut self, samples: &[f32], sensitivity: u8) -> Analysis {
        let keep = FFT.saturating_sub(samples.len());
        self.history.drain(..FFT - keep);
        self.history.extend_from_slice(&samples[samples.len().saturating_sub(FFT)..]);
        let range = 12.0 + 4.0 * sensitivity.clamp(1, 10) as f32;

        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
        let rms_db = db(rms);
        self.quiet_frames = if rms_db < SILENCE_DB { self.quiet_frames + 1 } else { 0 };
        if self.quiet_frames >= SILENT_AFTER {
            self.levels = [0.0; BANDS];
            self.loudness = 0.0;
            return Analysis { bands: self.levels, loudness: 0.0, silent: true };
        }

        let mut buf: Vec<Complex<f32>> = self.history.iter().zip(&self.window).map(|(s, w)| Complex::new(s * w, 0.0)).collect();
        self.fft.process(&mut buf);
        // amplitude of a full-scale sine = 1: scale by 2 / sum(window)
        let scale = 2.0 / self.window.iter().sum::<f32>();
        let band_db: Vec<f32> = self.bins.iter().zip(&self.tilt).map(|(&(lo, hi), t)| {
            db(buf[lo..hi.min(FFT / 2)].iter().map(|c| c.norm()).fold(0.0, f32::max) * scale) + t
        }).collect();

        let peak = band_db.iter().copied().fold(f32::MIN, f32::max);
        self.agc_db = peak.max(self.agc_db - AGC_DECAY_DB).max(AGC_FLOOR_DB);
        for (l, v) in self.levels.iter_mut().zip(&band_db) {
            *l = level(*v, self.agc_db, range).max(*l - FALL);
        }
        self.loud_agc_db = rms_db.max(self.loud_agc_db - AGC_DECAY_DB).max(AGC_FLOOR_DB);
        self.loudness = level(rms_db, self.loud_agc_db, range).max(self.loudness - FALL);
        Analysis { bands: self.levels, loudness: self.loudness, silent: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amp: f32, frame: usize) -> Vec<f32> {
        (0..CHUNK).map(|i| amp * (2.0 * std::f32::consts::PI * hz * (frame * CHUNK + i) as f32 / RATE as f32).sin()).collect()
    }
    fn run(a: &mut Analyzer, hz: f32, amp: f32, frames: usize) -> Analysis {
        let mut last = None;
        for f in 0..frames { last = Some(a.feed(&sine(hz, amp, f), 5)); }
        last.unwrap()
    }
    fn loudest(x: &Analysis) -> usize { (0..BANDS).max_by(|a, b| x.bands[*a].total_cmp(&x.bands[*b])).unwrap() }

    #[test]
    fn a_tone_lights_its_own_band() {
        let low = run(&mut Analyzer::new(), 100.0, 0.5, 10);
        let high = run(&mut Analyzer::new(), 5000.0, 0.5, 10);
        assert!(loudest(&low) < 4, "100 Hz is a bass band: {:?}", low.bands);
        assert!(loudest(&high) > 12, "5 kHz is a treble band: {:?}", high.bands);
        assert!(low.bands[loudest(&low)] > 0.9);
        assert!(low.bands[BANDS - 1] < 0.1, "far bands stay dark: {:?}", low.bands);
        assert!(!low.silent && low.loudness > 0.9);
    }

    #[test]
    fn automatic_gain_fills_the_range_for_quiet_music() {
        let quiet = run(&mut Analyzer::new(), 1000.0, 0.01, 10);
        assert!(quiet.bands[loudest(&quiet)] > 0.9, "{:?}", quiet.bands);
    }

    #[test]
    fn levels_fall_gradually() {
        let mut a = Analyzer::new();
        let loud = run(&mut a, 1000.0, 0.5, 10);
        let k = loudest(&loud);
        let after = a.feed(&sine(1000.0, 0.0005, 10), 5); // much quieter, not silence
        assert!(after.bands[k] >= loud.bands[k] - FALL - 1e-6, "{} → {}", loud.bands[k], after.bands[k]);
        assert!(after.bands[k] < loud.bands[k]);
    }

    #[test]
    fn silence_after_one_second() {
        let mut a = Analyzer::new();
        run(&mut a, 1000.0, 0.5, 5);
        let zeros = vec![0.0; CHUNK];
        for _ in 0..SILENT_AFTER - 1 { assert!(!a.feed(&zeros, 5).silent); }
        let s = a.feed(&zeros, 5);
        assert!(s.silent && s.loudness == 0.0 && s.bands.iter().all(|b| *b == 0.0));
        assert!(!a.feed(&sine(1000.0, 0.5, 0), 5).silent, "sound ends silence at once");
    }

    #[test]
    fn higher_sensitivity_shows_more() {
        // a loud bass tone plus a weak treble tone
        let mix = |f: usize| sine(100.0, 0.5, f).iter().zip(sine(8000.0, 0.005, f)).map(|(a, b)| a + b).collect::<Vec<_>>();
        let treble = |s: u8| { let mut a = Analyzer::new(); let mut x = None; for f in 0..10 { x = Some(a.feed(&mix(f), s)); } x.unwrap().bands[15] };
        assert!(treble(10) > treble(1), "{} vs {}", treble(10), treble(1));
    }
}
